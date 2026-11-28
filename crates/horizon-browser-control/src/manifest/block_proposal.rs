//! The bounds of one proposal of the front desk, and of one message that it
//! passes to a work item.
//!
//! A proposal holds text of a program: the title of each item becomes the name of
//! a workspace, the task becomes the first message of the orchestrator of that
//! workspace, and the folder becomes the folder that the orchestrator runs in. The
//! person reads all three before pressing anything, so every field is bounded
//! here, and a request outside the bounds is refused whole. Nothing is created
//! before the press, so a refusal changes nothing.

use super::block::{
    MAX_PROPOSAL_FOLDER_BYTES, MAX_PROPOSAL_ITEMS, MAX_PROPOSAL_TASK_CHARS, MAX_PROPOSAL_TITLE_CHARS,
    MAX_WORK_ITEM_MESSAGE_BYTES, ProposedItem,
};

/// Refusal code of a proposal whose items break a bound.
pub const INVALID_PROPOSAL: &str = "invalid_request";

/// Why one proposal or one message is not one that Horizon may hold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProposalRefusal {
    pub code: &'static str,
    pub message: String,
}

impl ProposalRefusal {
    fn new(message: impl Into<String>) -> Self {
        Self {
            code: INVALID_PROPOSAL,
            message: message.into(),
        }
    }
}

/// Check the items of one `work_item_propose` request: one to
/// [`MAX_PROPOSAL_ITEMS`] items, a title of 1 to [`MAX_PROPOSAL_TITLE_CHARS`]
/// characters on one line, a task of 1 to [`MAX_PROPOSAL_TASK_CHARS`] characters,
/// and, when it is given, an absolute folder of at most
/// [`MAX_PROPOSAL_FOLDER_BYTES`] bytes without a control character.
///
/// # Errors
/// Names the item and the field that breaks its rule, with
/// [`INVALID_PROPOSAL`].
pub fn check_items(items: &[ProposedItem]) -> Result<(), ProposalRefusal> {
    if items.is_empty() || items.len() > MAX_PROPOSAL_ITEMS {
        return Err(ProposalRefusal::new(format!(
            "work_item_propose takes 1 to {MAX_PROPOSAL_ITEMS} items, and this request holds {}",
            items.len()
        )));
    }
    for (index, item) in items.iter().enumerate() {
        check_title(index, &item.title)?;
        check_task(index, &item.task)?;
        if let Some(folder) = &item.folder {
            check_folder(index, folder)?;
        }
    }
    Ok(())
}

/// The title of one item: one line of visible text, because it becomes the name of
/// a workspace.
fn check_title(index: usize, title: &str) -> Result<(), ProposalRefusal> {
    let limit = MAX_PROPOSAL_TITLE_CHARS;
    if title.trim().is_empty() {
        return Err(ProposalRefusal::new(format!("items[{index}].title cannot be empty")));
    }
    if title.chars().count() > limit {
        return Err(ProposalRefusal::new(format!(
            "items[{index}].title has {} characters, above the {limit} character limit",
            title.chars().count()
        )));
    }
    if title.chars().any(char::is_control) {
        return Err(ProposalRefusal::new(format!(
            "items[{index}].title holds a control character; a title is one line of text"
        )));
    }
    Ok(())
}

/// The task of one item: text of up to the bound, which a later message carries.
fn check_task(index: usize, task: &str) -> Result<(), ProposalRefusal> {
    let limit = MAX_PROPOSAL_TASK_CHARS;
    if task.trim().is_empty() {
        return Err(ProposalRefusal::new(format!("items[{index}].task cannot be empty")));
    }
    if task.chars().count() > limit {
        return Err(ProposalRefusal::new(format!(
            "items[{index}].task has {} characters, above the {limit} character limit",
            task.chars().count()
        )));
    }
    Ok(())
}

/// The folder of one item: an absolute path of this computer. Horizon checks that
/// it is a directory only after the person's press, and only then makes it the
/// folder of the new workspace.
fn check_folder(index: usize, folder: &str) -> Result<(), ProposalRefusal> {
    let rule = format!(
        "items[{index}].folder must be an absolute path of at most {MAX_PROPOSAL_FOLDER_BYTES} bytes with no control character"
    );
    if folder.len() > MAX_PROPOSAL_FOLDER_BYTES
        || folder.chars().any(char::is_control)
        || !std::path::Path::new(folder).is_absolute()
    {
        return Err(ProposalRefusal::new(rule));
    }
    Ok(())
}

/// Check the text of one message that the front desk passes to a work item: 1
/// byte to the bound of one message of the person.
///
/// # Errors
/// Names the bound, with [`INVALID_PROPOSAL`].
pub fn check_message(text: &str) -> Result<(), ProposalRefusal> {
    if text.trim().is_empty() {
        return Err(ProposalRefusal::new("text cannot be empty"));
    }
    if text.len() > MAX_WORK_ITEM_MESSAGE_BYTES {
        return Err(ProposalRefusal::new(format!(
            "text has {} bytes, above the {MAX_WORK_ITEM_MESSAGE_BYTES} byte limit of one message",
            text.len()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{check_items, check_message};
    use crate::manifest::block::{MAX_PROPOSAL_ITEMS, MAX_PROPOSAL_TASK_CHARS, MAX_PROPOSAL_TITLE_CHARS, ProposedItem};

    fn item(title: &str, task: &str) -> ProposedItem {
        ProposedItem {
            title: title.to_owned(),
            task: task.to_owned(),
            folder: None,
        }
    }

    /// The refusal of `items`, or `None` when the request passes.
    fn refusal(items: &[ProposedItem]) -> Option<String> {
        check_items(items).err().map(|refusal| {
            assert_eq!(refusal.code, "invalid_request", "one refusal code for a proposal");
            refusal.message
        })
    }

    #[test]
    fn one_proposal_holds_one_to_six_items() {
        assert!(refusal(&[]).is_some_and(|message| message.contains("1 to 6")));
        let six: Vec<_> = (0..MAX_PROPOSAL_ITEMS)
            .map(|index| item(&format!("item {index}"), "do it"))
            .collect();
        assert_eq!(refusal(&six), None, "six items pass");
        let mut seven = six.clone();
        seven.push(item("item 6", "do it"));
        assert!(
            refusal(&seven).is_some_and(|message| message.contains("1 to 6")),
            "a seventh item is refused"
        );
    }

    #[test]
    fn a_title_is_one_line_of_one_to_sixty_characters() {
        assert!(refusal(&[item("   ", "do it")]).is_some_and(|message| message.contains("title cannot be empty")));
        let long = "t".repeat(MAX_PROPOSAL_TITLE_CHARS + 1);
        assert!(
            refusal(&[item(&long, "do it")]).is_some_and(|message| message.contains("61 characters")),
            "a title above the bound is refused"
        );
        let longest = "t".repeat(MAX_PROPOSAL_TITLE_CHARS);
        assert_eq!(refusal(&[item(&longest, "do it")]), None, "60 characters pass");
        assert!(
            refusal(&[item("two\nlines", "do it")]).is_some_and(|message| message.contains("control character")),
            "a title is one line"
        );
    }

    #[test]
    fn a_task_is_one_to_four_thousand_characters() {
        assert!(refusal(&[item("a", " ")]).is_some_and(|message| message.contains("task cannot be empty")));
        let long = "t".repeat(MAX_PROPOSAL_TASK_CHARS + 1);
        assert!(
            refusal(&[item("a", &long)]).is_some_and(|message| message.contains("4001 characters")),
            "a task above the bound is refused"
        );
        let longest = "t".repeat(MAX_PROPOSAL_TASK_CHARS);
        assert_eq!(refusal(&[item("a", &longest)]), None, "4 000 characters pass");
        // A task may hold line breaks: it is a message of the person.
        assert_eq!(refusal(&[item("a", "line one\nline two")]), None);
    }

    #[test]
    fn a_folder_is_an_absolute_path_of_this_computer() {
        let with = |folder: &str| ProposedItem {
            title: "a".to_owned(),
            task: "do it".to_owned(),
            folder: Some(folder.to_owned()),
        };
        assert_eq!(refusal(&[with("/home/person/work")]), None);
        assert!(
            refusal(&[with("work")]).is_some_and(|message| message.contains("absolute")),
            "a relative folder is refused whole"
        );
        assert!(
            refusal(&[with("/home/per\tson")]).is_some_and(|message| message.contains("control character")),
            "a folder holds no control character"
        );
    }

    #[test]
    fn one_message_is_one_byte_to_the_bound_of_a_message() {
        assert!(check_message("").is_err(), "an empty message sends nothing");
        assert!(check_message("  \n ").is_err(), "white space is no message");
        assert!(check_message("alpha").is_ok());
        let long = "x".repeat(256 * 1024 + 1);
        assert!(check_message(&long).is_err(), "a message above the bound is refused");
    }
}
