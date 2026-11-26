//! A suggested change, as github.com has one on a review comment: the
//! commented lines and what they should read. Commit suggestion makes the
//! change on the pull request's head branch, which is [`apply`] to the
//! file there. The composer and the thread that draw one are perch-app's
//! `review::suggestion`.

/// The lines a suggestion replaces, and the number of the first.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Original {
    pub first: u32,
    pub lines: Vec<String>,
}

/// A suggestion to commit on a pull request's head: the thread it is
/// in, the lines it replaces and what they become, and the commit's
/// message.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Commit {
    pub thread_id: String,
    pub path: String,
    pub original: Original,
    pub lines: Vec<String>,
    pub headline: String,
    pub body: String,
}

/// `file` with `original`'s lines made `lines`, as Commit suggestion
/// does on github.com. The file must still read `original` there, so a
/// change made since the suggestion was written is not overwritten. The
/// new lines end as the old ones did, `\r\n` or `\n`.
pub fn apply(file: &str, original: &Original, lines: &[String]) -> Result<String, String> {
    let changed = || "The lines this suggestion replaces have changed".to_string();
    let first = (original.first as usize)
        .checked_sub(1)
        .ok_or_else(changed)?;
    let last = first + original.lines.len();
    let mut rows: Vec<&str> = file.split_inclusive('\n').collect();
    if last > rows.len() || original.lines.is_empty() {
        return Err(changed());
    }
    let unended = |row: &str| {
        row.trim_end_matches('\n')
            .trim_end_matches('\r')
            .to_string()
    };
    if rows[first..last]
        .iter()
        .map(|r| unended(r))
        .ne(original.lines.iter().cloned())
    {
        return Err(changed());
    }
    // Only the file's last row can lack an ending; replacing it alone,
    // the row above says how the file's lines end.
    let ended = if rows[first].ends_with('\n') {
        rows[first]
    } else {
        first.checked_sub(1).map_or("", |i| rows[i])
    };
    let ending = if ended.ends_with("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    // The file's last line may have no ending; the new last line has none
    // either then.
    let open_end = last == rows.len() && !rows[last - 1].ends_with('\n');
    let mut new = String::new();
    for (i, line) in lines.iter().enumerate() {
        new.push_str(line);
        if !(open_end && i + 1 == lines.len()) {
            new.push_str(ending);
        }
    }
    if open_end && lines.is_empty() && first > 0 {
        // The last lines taken out: the one above them ends the file.
        let above = rows[first - 1];
        rows[first - 1] = above.trim_end_matches('\n').trim_end_matches('\r');
    }
    let mut out = String::with_capacity(file.len() + new.len());
    rows[..first].iter().for_each(|r| out.push_str(r));
    out.push_str(&new);
    rows[last..].iter().for_each(|r| out.push_str(r));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &[&str]) -> Vec<String> {
        text.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_suggestion_applies_over_the_lines_it_was_written_against() {
        let original = Original {
            first: 2,
            lines: lines(&["b", "c"]),
        };
        assert_eq!(
            apply("a\nb\nc\nd\n", &original, &lines(&["B"])),
            Ok("a\nB\nd\n".into())
        );
        assert_eq!(apply("a\nb\nc\nd\n", &original, &[]), Ok("a\nd\n".into()));
        assert_eq!(
            apply("a\r\nb\r\nc\r\n", &original, &lines(&["x", "y", "z"])),
            Ok("a\r\nx\r\ny\r\nz\r\n".into())
        );
        assert_eq!(
            apply("a\nb\nc", &original, &lines(&["x", "y"])),
            Ok("a\nx\ny".into()),
            "no ending at the end"
        );
        assert_eq!(apply("a\nb\nc", &original, &[]), Ok("a".into()));
        assert_eq!(
            apply(
                "a\r\nb\r\nc",
                &Original {
                    first: 3,
                    lines: lines(&["c"]),
                },
                &lines(&["x", "y"])
            ),
            Ok("a\r\nb\r\nx\r\ny".into()),
            "an unended last line takes the ending of the one above"
        );
        assert_eq!(
            apply(
                "c",
                &Original {
                    first: 1,
                    lines: lines(&["c"]),
                },
                &lines(&["x", "y"])
            ),
            Ok("x\ny".into())
        );
        assert!(
            apply("a\nb2\nc\n", &original, &[]).is_err(),
            "changed since"
        );
        assert!(apply("a\nb\n", &original, &[]).is_err(), "shorter now");
    }
}
