//! Reading and writing PGN, the standard text format for chess games.
//!
//! `parse` only splits the text into moves; whether those moves are legal is for the
//! caller to find out by playing them.

/// What a PGN file says, before any move has been checked.
#[derive(Debug, Default, PartialEq)]
pub struct Pgn {
    /// Starting position, if the game did not begin from the usual one.
    pub fen: Option<String>,
    /// Moves in SAN, without numbers, comments or annotations.
    pub moves: Vec<String>,
    /// "1-0", "0-1", "1/2-1/2" or "*", if the text gives one.
    pub result: Option<String>,
}

const RESULTS: [&str; 4] = ["1-0", "0-1", "1/2-1/2", "*"];

/// Value of a `[Name "Value"]` tag line, if `line` is one for `name`.
fn tag<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let rest = line.strip_prefix('[')?.trim_start();
    let rest = rest.strip_prefix(name)?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let open = rest.find('"')?;
    let close = rest.rfind('"')?;
    (close > open).then(|| &rest[open + 1..close])
}

pub fn parse(text: &str) -> Pgn {
    let mut out = Pgn::default();
    let mut body = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('%') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            if let Some(v) = tag(line, "FEN") {
                out.fen = Some(v.trim().to_string());
            } else if let Some(v) = tag(line, "Result") {
                if RESULTS.contains(&v) {
                    out.result = Some(v.to_string());
                }
            }
            continue;
        }
        body.push_str(line);
        body.push('\n');
    }

    // Drop {comments}, ;comments to end of line, and (variations), which may nest.
    let mut clean = String::with_capacity(body.len());
    let (mut brace, mut paren, mut rest_of_line) = (false, 0u32, false);
    for c in body.chars() {
        if rest_of_line {
            rest_of_line = c != '\n';
        } else if brace {
            brace = c != '}';
        } else {
            match c {
                '{' => brace = true,
                ';' => rest_of_line = true,
                '(' => paren += 1,
                ')' => paren = paren.saturating_sub(1),
                _ if paren == 0 => clean.push(c),
                _ => {}
            }
        }
        if !brace && !rest_of_line && paren == 0 && matches!(c, '}' | ')' | '\n') {
            clean.push(' ');
        }
    }

    for token in clean.split_whitespace() {
        if RESULTS.contains(&token) {
            if token != "*" || out.result.is_none() {
                out.result = Some(token.to_string());
            }
            continue;
        }
        if token.starts_with('$') {
            continue;
        }
        // "12." and "12..." may stand alone or be glued to the move: "12.Nf3".
        // Digits alone are not a number prefix: "0-0" is castling.
        let after_digits = token.trim_start_matches(|c: char| c.is_ascii_digit());
        let san = if after_digits.starts_with('.') { after_digits.trim_start_matches('.') } else { token };
        let san = san.trim_end_matches(['!', '?']);
        if san.is_empty() || san == "e.p." {
            continue;
        }
        out.moves.push(match san.trim_end_matches(['+', '#']) {
            "0-0" => san.replacen("0-0", "O-O", 1),
            "0-0-0" => san.replacen("0-0-0", "O-O-O", 1),
            _ => san.to_string(),
        });
    }
    out
}

/// Write a game. `start_fen` is given only when the game began from an unusual position.
pub fn format(tags: &[(&str, &str)], start_fen: Option<&str>, moves: &[String], result: &str) -> String {
    let mut out = String::new();
    for (k, v) in tags {
        out.push_str(&format!("[{k} \"{}\"]\n", v.replace('\\', "\\\\").replace('"', "\\\"")));
    }
    out.push_str(&format!("[Result \"{result}\"]\n"));
    let (mut number, mut black) = (1u32, false);
    if let Some(fen) = start_fen {
        out.push_str(&format!("[SetUp \"1\"]\n[FEN \"{fen}\"]\n"));
        let f: Vec<&str> = fen.split_whitespace().collect();
        black = f.get(1) == Some(&"b");
        number = f.get(5).and_then(|n| n.parse().ok()).unwrap_or(1);
    }
    out.push('\n');

    let mut words: Vec<String> = Vec::new();
    for (i, san) in moves.iter().enumerate() {
        if !black {
            words.push(format!("{number}."));
        } else if i == 0 {
            words.push(format!("{number}..."));
        }
        words.push(san.clone());
        if black {
            number += 1;
        }
        black = !black;
    }
    words.push(result.to_string());
    let mut width = 0;
    for w in words {
        if width > 0 && width + 1 + w.len() > 80 {
            out.push('\n');
            width = 0;
        } else if width > 0 {
            out.push(' ');
            width += 1;
        }
        width += w.len();
        out.push_str(&w);
    }
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_tags_comments_variations_and_annotations() {
        let text = r#"[Event "Casual game"]
[White "A \"Tal\" fan"]
[Result "1-0"]

1. e4 {best by test} e5 2. Nf3 (2. f4 exf4 (2... d5) 3. Nf3) 2... Nc6 $1 3.Bb5!? a6 ; the Morphy
4. Ba4 Nf6 5. 0-0 Be7?! 1-0
"#;
        let p = parse(text);
        assert_eq!(p.moves, ["e4", "e5", "Nf3", "Nc6", "Bb5", "a6", "Ba4", "Nf6", "O-O", "Be7"]);
        assert_eq!(p.result.as_deref(), Some("1-0"));
        assert_eq!(p.fen, None);
    }

    #[test]
    fn reads_a_bare_move_list_and_a_fen_tag() {
        let p = parse("e4 e5 Qh5 Nc6 Bc4 Nf6 Qxf7#");
        assert_eq!(p.moves.len(), 7);
        assert_eq!(p.moves[6], "Qxf7#");
        assert_eq!(p.result, None);
        let p = parse("[SetUp \"1\"]\n[FEN \"4k3/8/8/8/8/8/8/R3K3 b - - 3 40\"]\n\n40... Kd7 41. Ra7+ *");
        assert_eq!(p.fen.as_deref(), Some("4k3/8/8/8/8/8/8/R3K3 b - - 3 40"));
        assert_eq!(p.moves, ["Kd7", "Ra7+"]);
        assert_eq!(p.result.as_deref(), Some("*"));
    }

    #[test]
    fn writes_what_it_reads() {
        let moves: Vec<String> = ["e4", "e5", "Nf3", "Nc6", "Bb5"].iter().map(|s| s.to_string()).collect();
        let text = format(&[("Event", "Test"), ("White", "A \"B\"")], None, &moves, "*");
        assert!(text.starts_with("[Event \"Test\"]\n[White \"A \\\"B\\\"\"]\n[Result \"*\"]\n\n1. e4 e5 2. Nf3 Nc6 3. Bb5 *\n"), "{text}");
        assert_eq!(parse(&text).moves, moves);

        let fen = "4k3/8/8/8/8/8/8/R3K3 b - - 3 40";
        let moves: Vec<String> = ["Kd7", "Ra7+", "Kc6"].iter().map(|s| s.to_string()).collect();
        let text = format(&[], Some(fen), &moves, "1/2-1/2");
        assert!(text.ends_with("\n40... Kd7 41. Ra7+ Kc6 1/2-1/2\n"), "{text}");
        let back = parse(&text);
        assert_eq!((back.fen.as_deref(), back.moves, back.result.as_deref()), (Some(fen), moves, Some("1/2-1/2")));
    }

    #[test]
    fn wraps_long_games() {
        let moves: Vec<String> = (0..120).map(|i| if i % 2 == 0 { "Nf3".to_string() } else { "Nf6".to_string() }).collect();
        let text = format(&[], None, &moves, "*");
        assert!(text.lines().all(|l| l.len() <= 80));
        assert_eq!(parse(&text).moves.len(), 120);
    }
}
