//! Finding the repositories where a bot has open pull requests, for Upkeep's setup.
use super::{Client, Error, Result, Secret};
use serde::Deserialize;
use std::collections::BTreeMap;

/// GitHub's search answers at most 1,000 results: ten pages of 100.
const MAX_PAGES: u32 = 10;

impl Client {
    /// The open pull requests that `author` (such as `app/dependabot`) opened in repositories
    /// of `owners`, counted per lowercase `owner/name`. Only repositories in `reachable`
    /// count, so a repository the app cannot reach is never offered. The counts order a list
    /// and may be a lower bound: GitHub answers at most 1,000 results per search, and it
    /// can end a slow search early with `incomplete_results`.
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
        for owner in owners {
            let mut read = 0_u64;
            for page in 1..=MAX_PAGES {
                let path = format!(
                    "/search/issues?q=is%3Apr+is%3Aopen+author%3A{}+user%3A{owner}&per_page=100&page={page}",
                    author.replace('/', "%2F")
                );
                let answer: Answer = self.get_search(token, &path)?;
                read += answer.items.len() as u64;
                let last = answer.items.len() < 100 || read >= answer.total_count;
                for item in answer.items {
                    let name = item
                        .repository_url
                        .strip_prefix("https://api.github.com/repos/")
                        .ok_or(Error::InvalidResponse)?
                        .to_ascii_lowercase();
                    if reachable.contains(&name) {
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
        let mut first = vec!["Acme/Web"; 99];
        first.push("acme/hidden");
        let (client, requests, task) = github(vec![
            (200, page(102, &first)),
            (200, page(102, &["acme/web", "acme/api"])),
            (200, page(1, &["other/tool"])),
        ]);
        let counts = client
            .open_pull_requests_by(
                &Secret::new("ghu_synthetic".into()),
                "app/dependabot",
                &["acme".into(), "other".into()],
                &["acme/web".into(), "acme/api".into(), "other/tool".into()],
            )
            .unwrap();
        task.join().unwrap();
        assert_eq!(
            counts,
            BTreeMap::from([
                ("acme/api".into(), 1),
                ("acme/web".into(), 100),
                ("other/tool".into(), 1)
            ]),
            "lowercased, and the repository the app cannot reach is left out"
        );
        let lines: Vec<String> = requests.lock().unwrap().iter().map(|(line, _)| line.clone()).collect();
        assert_eq!(
            lines,
            [
                "GET /search/issues?q=is%3Apr+is%3Aopen+author%3Aapp%2Fdependabot+user%3Aacme&per_page=100&page=1 HTTP/1.1",
                "GET /search/issues?q=is%3Apr+is%3Aopen+author%3Aapp%2Fdependabot+user%3Aacme&per_page=100&page=2 HTTP/1.1",
                "GET /search/issues?q=is%3Apr+is%3Aopen+author%3Aapp%2Fdependabot+user%3Aother&per_page=100&page=1 HTTP/1.1",
            ]
        );
    }

    #[test]
    fn reads_a_page_of_long_descriptions() {
        let body = "r".repeat(60_000);
        let items: Vec<Value> = (0..100)
            .map(|_| json!({"repository_url": "https://api.github.com/repos/acme/web", "body": body}))
            .collect();
        let (client, _requests, task) = github(vec![(200, json!({"total_count": 100, "items": items}))]);
        let counts = client
            .open_pull_requests_by(
                &Secret::new("ghu_synthetic".into()),
                "app/dependabot",
                &["acme".into()],
                &["acme/web".into()],
            )
            .unwrap();
        task.join().unwrap();
        assert_eq!(counts["acme/web"], 100, "6 MB of release notes is one page");
    }

    #[test]
    fn stops_at_the_last_page_github_answers() {
        let full = vec!["acme/web"; 100];
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
    fn an_item_outside_the_api_is_a_malformed_answer() {
        let (client, _requests, task) = github(vec![(
            200,
            json!({"total_count": 1, "items": [{"repository_url": "https://example.com/repos/acme/web"}]}),
        )]);
        let result = client.open_pull_requests_by(
            &Secret::new("ghu_synthetic".into()),
            "app/dependabot",
            &["acme".into()],
            &["acme/web".into()],
        );
        task.join().unwrap();
        assert_eq!(result, Err(Error::InvalidResponse));
    }
}
