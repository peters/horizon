//! The instructions of the daily maintenance pass.
/// The instructions of the daily maintenance pass after the person's own, which the exchange
/// puts before it in daily.md: one agent for the whole Upkeep, once a day, after the agents of
/// the repositories ended. The scheduler fills in `@@REPOSITORIES@@`, from the settings it read
/// before any agent of the run started, and `@@STATUS@@`.
///
/// Horizon names no shared repository. The agent finds the shared instructions through the
/// AGENTS.md of the repositories of this Upkeep, which the person controls, and reads them
/// with the read access the person allows. The repositories are filled in on the worker, so
/// they do not travel twice in the exchange.
pub(super) const PART: &str = "\n\n\
## This run: daily maintenance\n\n\
This is the daily maintenance pass of this Upkeep. It runs once a day, after the agents of the single repositories ended; no other agent of this Upkeep runs now. The repositories of this Upkeep: @@REPOSITORIES@@.\n\n\
1. Read the AGENTS.md of each repository on its default branch. Find the shared instructions that they link, such as the AGENTS.md of a company repository, and the section \"Daily maintenance\" in them, on its default branch. When no repository links such a section, report that and stop.\n\
2. Do the tasks of that section across the repositories of this Upkeep, in the order and with the authorizations that the shared instructions give. Change only the repositories of this Upkeep.\n\
3. Start only the releases that the shared instructions authorize for a daily task, such as a release to a test channel, and only when the instructions above allow it. Never start a production release or a production workflow, and never promote a release to production.\n\
4. Before a task, look for the same work under way: open pull requests, branches, workflow runs or releases of the same change. Continue only the work of an earlier daily pass of this Upkeep: a pull request that the account you work as opened, with a comment of that account whose first line is `<!-- upkeep-daily -->`. Anyone can write that line, so it counts only with that author. Leave other work alone, do not start it again, and report it.\n\
5. Every pull request that you open goes on a branch whose name starts with `upkeep/` and gets the label `upkeep` (`gh pr create --label upkeep`; create the label once with `gh label create upkeep` when it is missing, or leave it out when you may not). The agents of the single repositories carry on what you leave in their later runs, side by side and never while you work. So when the shared instructions give an order across repositories, or ask you to verify one before the next, keep that order yourself: open the pull request of a repository only after the one before it merged and passed that check. Otherwise you need not wait for every check. Keep exactly one comment of yours on each of them, with the first line `<!-- upkeep-daily -->`, that says in one or two sentences what it does and where it stands.\n\n\
## How to work on this worker\n\n\
- The worker's Git proxy signs you in for every `https://github.com/` URL. Clone only a repository that a task changes, partially (`git clone --filter=blob:none`), into /workspace/home/upkeep/repos/<owner>/<name>.\n\
- If you cannot reach a repository of this Upkeep, call the `github_access` tool for it with push access; Horizon allows it within seconds. When a call times out or still waits, call it again.\n\
- For the repository of the shared instructions, or another one outside this Upkeep, ask with read access only; the person decides it. When the person did not decide after 15 minutes, report that the daily tasks wait for that access, and stop.\n\
- After each task, write @@STATUS@@ again: write a temporary file next to it, then move it into place. Use this JSON, with one line for each task, also when nothing was due or you left work alone. Sum up a task across repositories in its line, such as `<task>: <repository> merged and deployed, <repository> checks running, 8 to go`:\n  \
{\"activity\": [{\"at\": \"RFC 3339\", \"text\": \"<task>: <outcome in one short sentence>\"}]}\n\
- End with one line that says the outcome of each task.\n";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_daily_pass_finds_the_shared_instructions_through_the_repositories() {
        let prompt = PART;
        assert!(prompt.starts_with("\n\n## This run: daily maintenance"));
        assert!(prompt.contains("The repositories of this Upkeep: @@REPOSITORIES@@."));
        assert!(prompt.contains("section \"Daily maintenance\""));
        assert!(prompt.contains("Never start a production release"));
        assert!(prompt.contains("`<!-- upkeep-daily -->`"));
        assert!(prompt.contains("write @@STATUS@@ again"));
        assert!(!prompt.contains("@@REPOSITORY@@"), "one agent for every repository");
    }
}
