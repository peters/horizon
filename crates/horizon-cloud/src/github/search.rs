//! Finding the repositories where a bot has open pull requests, for Upkeep's setup.
use super::{Client, Error, Result, Secret};
use serde::Deserialize;
use std::collections::BTreeMap;

/// Pull requests per search page. A page of 50 stays within the read limit even when
/// every description is as long and as escaped as GitHub allows.
const PAGE: usize = 50;

/// GitHub's search answers at most 1,000 results: twenty pages of 50.
const MAX_PAGES: u32 = 20;

/// The longest search query GitHub takes, in characters.
const MAX_QUERY: usize = 256;

/// The most owners in one query; GitHub refuses a query with too many `user:` qualifiers.
const MAX_OWNERS: usize = 10;

impl Client {
    /// The open pull requests that `author` (such as `app/dependabot`) opened in repositories
    /// of `owners`, counted per lowercase `owner/name`. Only repositories in `reachable`
    /// count, so a repository the app cannot reach is never offered. One search covers as
    /// many owners as its query holds, which keeps the number of searches under GitHub's
    /// search rate limit. The counts order a list and may be a lower bound: GitHub answers
    /// at most 1,000 results per search, and it can end a slow search early with
    /// `incomplete_results`.
    /// # Errors
    /// An invalid author or owner, transport failures, a refused token, or a malformed answer.
    pub fn open_pull_requests_by(
        &self,
        token: &Secret,
        author: &str,
        owners: &[String],
        reachable: &[String],
    ) -> Result<BTreeMap<String, u32>> {
        #[derive(Deserialize)]
        struct Answer {
            total_count: u64,
            items: Vec<Item>,
        }
        #[derive(Deserialize)]
        struct Item {
            repository_url: String,
        }
        if !valid_name(author.trim_start_matches("app/")) || !owners.iter().all(|owner| valid_name(owner)) {
            return Err(Error::InvalidResponse);
        }
        let mut counts = BTreeMap::new();
        // Each owner is searched once, however often it is named.
        let owners: std::collections::BTreeSet<String> =
            owners.iter().map(|owner| owner.to_ascii_lowercase()).collect();
        for query in queries(author, &owners) {
            let mut read = 0_u64;
            for page in 1..=MAX_PAGES {
                let path = format!("/search/issues?q={query}&per_page={PAGE}&page={page}");
                let answer: Answer = self.get_search(token, &path)?;
                read += answer.items.len() as u64;
                let last = answer.items.len() < PAGE || read >= answer.total_count;
                for item in answer.items {
                    let name = item
                        .repository_url
                        .strip_prefix("https://api.github.com/repos/")
                        .filter(|name| super::valid_repository(name))
                        .ok_or(Error::InvalidResponse)?
                        .to_ascii_lowercase();
                    if reachable.iter().any(|known| known.eq_ignore_ascii_case(&name)) {
                        *counts.entry(name).or_insert(0) += 1;
                    }
                }
                if last {
                    break;
                }
            }
        }
        Ok(counts)
    }
}

/// The search queries for `owners`, URL-encoded, each within GitHub's query length.
/// Several `user:` qualifiers in one query match any of them.
fn queries(author: &str, owners: &std::collections::BTreeSet<String>) -> Vec<String> {
    let base = format!("is:pr is:open author:{author}");
    let mut queries: Vec<String> = Vec::new();
    let mut current = base.clone();
    let mut count = 0;
    for owner in owners {
        let qualifier = format!(" user:{owner}");
        if (current.len() + qualifier.len() > MAX_QUERY || count == MAX_OWNERS) && current != base {
            queries.push(std::mem::replace(&mut current, base.clone()));
            count = 0;
        }
        current.push_str(&qualifier);
        count += 1;
    }
    if current != base {
        queries.push(current);
    }
    queries
        .iter()
        .map(|query| query.replace(':', "%3A").replace('/', "%2F").replace(' ', "+"))
        .collect()
}

/// A GitHub login or app slug.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 100
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::super::tests::github;
    use super::*;
    use serde_json::{Value, json};

    fn page(total: u64, names: &[&str]) -> Value {
        let items: Vec<Value> = names
            .iter()
            .map(|name| json!({"repository_url": format!("https://api.github.com/repos/{name}")}))
            .collect();
        json!({"total_count": total, "incomplete_results": false, "items": items})
    }

    #[test]
    fn counts_open_pull_requests_per_reachable_repository_on_every_page() {
        let mut first = vec!["Acme/Web"; PAGE - 1];
        first.push("acme/hidden");
        let (client, requests, task) = github(vec![
            (200, page(102, &first)),
            (200, page(102, &["acme/web", "acme/api", "other/tool"])),
        ]);
        let counts = client
            .open_pull_requests_by(
                &Secret::new("ghu_synthetic".into()),
                "app/dependabot",
                &["acme".into(), "other".into(), "Acme".into(), "acme".into()],
                &["acme/web".into(), "Acme/API".into(), "other/tool".into()],
            )
            .unwrap();
        task.join().unwrap();
        assert_eq!(
            counts,
            BTreeMap::from([
                ("acme/api".into(), 1),
                ("acme/web".into(), 50),
                ("other/tool".into(), 1)
            ]),
            "lowercased, each owner searched once, and the repository the app cannot reach is left out"
        );
        let lines: Vec<String> = requests.lock().unwrap().iter().map(|(line, _)| line.clone()).collect();
        assert_eq!(
            lines,
            [
                "GET /search/issues?q=is%3Apr+is%3Aopen+author%3Aapp%2Fdependabot+user%3Aacme+user%3Aother&per_page=50&page=1 HTTP/1.1",
                "GET /search/issues?q=is%3Apr+is%3Aopen+author%3Aapp%2Fdependabot+user%3Aacme+user%3Aother&per_page=50&page=2 HTTP/1.1",
            ]
        );
    }

    #[test]
    fn reads_a_page_of_long_descriptions() {
        // Control characters are escaped as six bytes each: the largest page GitHub sends.
        let body = "\u{1}".repeat(65_536);
        let items: Vec<Value> = (0..PAGE)
            .map(|_| json!({"repository_url": "https://api.github.com/repos/acme/web", "body": body}))
            .collect();
        let (client, _requests, task) = github(vec![(200, json!({"total_count": PAGE, "items": items}))]);
        let counts = client
            .open_pull_requests_by(
                &Secret::new("ghu_synthetic".into()),
                "app/dependabot",
                &["acme".into()],
                &["acme/web".into()],
            )
            .unwrap();
        task.join().unwrap();
        assert_eq!(
            counts["acme/web"], 50,
            "a page of the longest escaped descriptions is read"
        );
    }

    #[test]
    fn many_owners_share_few_searches_within_the_query_length() {
        let owners: std::collections::BTreeSet<String> = (0..60).map(|n| format!("owner-number-{n:02}")).collect();
        let queries = queries("app/dependabot", &owners);
        assert_eq!(
            queries.len(),
            6,
            "60 owners in queries of at most 256 characters and 10 owners"
        );
        let short: std::collections::BTreeSet<String> = (b'a'..=b'q').map(|c| char::from(c).to_string()).collect();
        assert_eq!(
            super::queries("app/dependabot", &short).len(),
            2,
            "17 short owners exceed the owner cap, not the length"
        );
        let decoded: Vec<String> = queries
            .iter()
            .map(|query| query.replace("%3A", ":").replace("%2F", "/").replace('+', " "))
            .collect();
        assert!(decoded.iter().all(|query| query.len() <= MAX_QUERY && query.starts_with("is:pr is:open author:app/dependabot user:")));
        assert_eq!(
            decoded
                .iter()
                .map(|query| query.matches(" user:").count())
                .sum::<usize>(),
            60
        );
    }

    #[test]
    fn stops_at_the_last_page_github_answers() {
        let full = vec!["acme/web"; PAGE];
        let (client, requests, task) = github((0..MAX_PAGES).map(|_| (200, page(5000, &full))).collect());
        let counts = client
            .open_pull_requests_by(
                &Secret::new("ghu_synthetic".into()),
                "app/dependabot",
                &["acme".into()],
                &["acme/web".into()],
            )
            .unwrap();
        task.join().unwrap();
        assert_eq!(counts["acme/web"], 1000, "a lower bound past GitHub's 1,000 results");
        assert_eq!(requests.lock().unwrap().len(), MAX_PAGES as usize);
    }

    #[test]
    fn refuses_a_name_that_could_change_the_query_without_asking_github() {
        let client = Client::new();
        let token = Secret::new("ghu_synthetic".into());
        for (author, owner) in [
            ("app/dependabot+is:closed", "acme"),
            ("app/dependabot", "acme user:other"),
            ("", "acme"),
        ] {
            assert_eq!(
                client.open_pull_requests_by(&token, author, &[owner.into()], &[]),
                Err(Error::InvalidResponse),
                "{author} {owner}"
            );
        }
    }

    #[test]
    fn an_item_that_is_not_a_repository_of_the_api_is_a_malformed_answer() {
        for url in [
            "https://example.com/repos/acme/web",
            "https://api.github.com/repos/acme/web/issues",
            "https://api.github.com/repos/acme",
        ] {
            let (client, _requests, task) = github(vec![(
                200,
                json!({"total_count": 1, "items": [{"repository_url": url}]}),
            )]);
            let result = client.open_pull_requests_by(
                &Secret::new("ghu_synthetic".into()),
                "app/dependabot",
                &["acme".into()],
                &["acme/web".into()],
            );
            task.join().unwrap();
            assert_eq!(result, Err(Error::InvalidResponse), "{url}");
        }
    }
}
