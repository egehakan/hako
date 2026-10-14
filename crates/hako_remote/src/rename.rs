//! The names a remote client gives tabs and folders.

use crate::protocol::RpcError;

pub const MAX_TAB_TITLE_CHARS: usize = 200;
pub const MAX_FOLDER_NAME_CHARS: usize = 100;

/// The name to keep for a requested `name` of at most `max_chars` characters once trimmed, or
/// `None` to go back to the automatic one.
pub fn chosen_name(name: Option<&str>, max_chars: usize) -> Result<Option<String>, RpcError> {
    let Some(name) = name else {
        return Ok(None);
    };
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(RpcError::bad_request("The name is empty"));
    }
    if trimmed.chars().count() > max_chars {
        return Err(RpcError::bad_request(format!(
            "The name is longer than {max_chars} characters"
        )));
    }
    Ok(Some(trimmed.to_owned()))
}

/// Refuses an id that no tab could have, which tells a client that probes for renaming apart
/// from an app that does not rename.
pub fn well_formed_tab_id(id: &str) -> Result<(), RpcError> {
    if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(RpcError::bad_request(format!("Not a tab id: {id}")));
    }
    Ok(())
}

#[cfg(test)]
#[path = "rename_tests.rs"]
mod tests;
