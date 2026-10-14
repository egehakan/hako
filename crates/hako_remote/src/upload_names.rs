//! The names an uploaded file may arrive with and the names it is given: the check of a name a
//! client sends, the numbered names that keep both files when a name is taken, and the names in
//! the uploads area, which can be typed into a shell as they are.

/// The longest name, in bytes, that a file system takes.
pub const MAX_NAME_BYTES: usize = 255;

/// The most characters of a sent name that a name in the uploads area keeps, before its
/// extension.
const UPLOADS_STEM_CHARS: usize = 64;
const UPLOADS_EXTENSION_CHARS: usize = 8;

/// Whether `name` is one path segment a file can be given exactly.
pub fn is_valid(name: &str) -> bool {
    !matches!(name, "" | "." | "..") && !name.contains(['/', '\0']) && name.len() <= MAX_NAME_BYTES
}

/// The `n`th name for `name` when it is taken: `name` itself for 1, then `stem (2).ext`,
/// `stem (3).ext`, …, with the stem shortened to keep the name within [`MAX_NAME_BYTES`].
pub fn numbered(name: &str, n: u32) -> String {
    if n <= 1 {
        return name.to_owned();
    }
    let suffix = format!(" ({n})");
    let (stem, extension) = match name.rfind('.') {
        Some(dot) if dot > 0 && name.len() - dot + suffix.len() < MAX_NAME_BYTES => {
            name.split_at(dot)
        }
        Some(_) | None => (name, ""),
    };
    let room = MAX_NAME_BYTES - suffix.len() - extension.len();
    format!("{}{suffix}{extension}", truncated(stem, room))
}

/// The `n`th name for a directory called `name` when it is taken: `name` itself for 1, then
/// `name (2)`, `name (3)`, …, shortened to keep the name within [`MAX_NAME_BYTES`].
pub fn numbered_directory(name: &str, n: u32) -> String {
    if n <= 1 {
        return name.to_owned();
    }
    let suffix = format!(" ({n})");
    format!("{}{suffix}", truncated(name, MAX_NAME_BYTES - suffix.len()))
}

/// `text` cut to at most `bytes` bytes, at a character boundary.
fn truncated(text: &str, bytes: usize) -> &str {
    let mut end = bytes.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// The name a file sent as `name` gets in the uploads area. It holds nothing but ASCII letters,
/// digits, `.`, `_` and `-`, and starts with neither `.` nor `-`.
pub fn for_uploads_area(name: &str) -> UploadsAreaName {
    let (rest, extension) = match name.rfind('.') {
        Some(dot)
            if dot > 0
                && (1..=UPLOADS_EXTENSION_CHARS).contains(&(name.len() - dot - 1))
                && name[dot + 1..]
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric()) =>
        {
            (&name[..dot], Some(name[dot + 1..].to_ascii_lowercase()))
        }
        Some(_) | None => (name, None),
    };

    let mut replaced = String::with_capacity(rest.len());
    for character in rest.chars() {
        let character = if character.is_ascii_alphanumeric() || matches!(character, '.' | '_') {
            character
        } else {
            '-'
        };
        if !(character == '-' && replaced.ends_with('-')) {
            replaced.push(character);
        }
    }
    let trimmed = replaced.trim_matches(['-', '.']);
    // Every character left is ASCII, so characters and bytes are the same here.
    let stem = trimmed[..trimmed.len().min(UPLOADS_STEM_CHARS)].trim_end_matches(['-', '.']);
    UploadsAreaName {
        stem: if stem.is_empty() { "file" } else { stem }.to_owned(),
        extension,
    }
}

/// A name in the uploads area, from which the names to use when it is taken are made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UploadsAreaName {
    stem: String,
    /// Without its dot, and in lower case.
    extension: Option<String>,
}

impl UploadsAreaName {
    /// The `n`th name to try: the name itself for 1, then `stem-2.ext`, `stem-3.ext`, ….
    pub fn numbered(&self, n: u32) -> String {
        let number = if n <= 1 {
            String::new()
        } else {
            format!("-{n}")
        };
        match &self.extension {
            Some(extension) => format!("{}{number}.{extension}", self.stem),
            None => format!("{}{number}", self.stem),
        }
    }
}

#[cfg(test)]
#[path = "upload_names_tests.rs"]
mod tests;
