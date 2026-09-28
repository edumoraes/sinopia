//! A text's runs, worked on: the style at a place, a change laid over a
//! stretch, the runs following an edit, and keeping them tidy.
//!
//! Every operation goes through the same two doors — the runs spread out
//! to one style a character ([`expand`]), and gathered back ([`compress`])
//! — so there is one place that says what tidy is: no run saying what the
//! text already says, none saying nothing, and no two side by side saying
//! the same. A text a page long is a few thousand characters; spreading
//! it out costs less than getting a split at a run's edge wrong.

use crate::doc::{Run, RunStyle, TextStyle};

/// One style a character, for a text `n` characters long: what its run
/// sets, or nothing.
pub fn expand(runs: &[Run], n: usize) -> Vec<RunStyle> {
    let mut out = vec![RunStyle::default(); n];
    for r in runs {
        for s in out.iter_mut().take(r.end.min(n)).skip(r.start) {
            *s = r.style.clone();
        }
    }
    out
}

/// `style` with every field that says what `base` already says taken off.
fn without_base(style: &RunStyle, base: &TextStyle) -> RunStyle {
    RunStyle {
        font: style.font.clone().filter(|v| *v != base.font),
        size: style.size.filter(|&v| v != base.size),
        bold: style.bold.filter(|&v| v != base.bold),
        italic: style.italic.filter(|&v| v != base.italic),
        underline: style.underline.filter(|&v| v != base.underline),
        strike: style.strike.filter(|&v| v != base.strike),
        color: style.color.clone().filter(|v| *v != base.color),
        tracking: style.tracking.filter(|&v| v != base.tracking),
    }
}

/// Gathers one style a character back into runs, tidy.
pub fn compress(styles: &[RunStyle], base: &TextStyle) -> Vec<Run> {
    let mut out: Vec<Run> = Vec::new();
    for (i, style) in styles.iter().enumerate() {
        let style = without_base(style, base);
        if style.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.end == i && last.style == style => last.end = i + 1,
            _ => out.push(Run {
                start: i,
                end: i + 1,
                style,
            }),
        }
    }
    out
}

/// The runs as they would be written: tidy.
pub fn tidy(runs: &[Run], n: usize, base: &TextStyle) -> Vec<Run> {
    compress(&expand(runs, n), base)
}

/// How character `i` is set.
pub fn style_at(base: &TextStyle, runs: &[Run], i: usize) -> TextStyle {
    runs.iter()
        .find(|r| r.start <= i && i < r.end)
        .map_or_else(|| base.clone(), |r| r.style.over(base))
}

/// `patch` laid over what `over` sets: its fields where it has them.
pub fn merge(over: &RunStyle, patch: &RunStyle) -> RunStyle {
    RunStyle {
        font: patch.font.clone().or_else(|| over.font.clone()),
        size: patch.size.or(over.size),
        bold: patch.bold.or(over.bold),
        italic: patch.italic.or(over.italic),
        underline: patch.underline.or(over.underline),
        strike: patch.strike.or(over.strike),
        color: patch.color.clone().or_else(|| over.color.clone()),
        tracking: patch.tracking.or(over.tracking),
    }
}

/// Characters `a..b` set as `patch` says, over whatever they were.
pub fn apply(runs: &[Run], n: usize, base: &TextStyle, a: usize, b: usize, patch: &RunStyle) -> Vec<Run> {
    let mut styles = expand(runs, n);
    for s in styles.iter_mut().take(b.min(n)).skip(a) {
        *s = merge(s, patch);
    }
    compress(&styles, base)
}

/// What `after` sets differently from `before`, among what a run may set.
pub fn diff(before: &TextStyle, after: &TextStyle) -> RunStyle {
    RunStyle {
        font: (after.font != before.font).then(|| after.font.clone()),
        size: (after.size != before.size).then_some(after.size),
        bold: (after.bold != before.bold).then_some(after.bold),
        italic: (after.italic != before.italic).then_some(after.italic),
        underline: (after.underline != before.underline).then_some(after.underline),
        strike: (after.strike != before.strike).then_some(after.strike),
        color: (after.color != before.color).then(|| after.color.clone()),
        tracking: (after.tracking != before.tracking).then_some(after.tracking),
    }
}

/// Every run with the fields `patch` sets taken off: the whole text takes
/// them now, from its own style.
pub fn strip(runs: &[Run], n: usize, base: &TextStyle, patch: &RunStyle) -> Vec<Run> {
    let styles: Vec<RunStyle> = expand(runs, n)
        .into_iter()
        .map(|s| RunStyle {
            font: s.font.filter(|_| patch.font.is_none()),
            size: s.size.filter(|_| patch.size.is_none()),
            bold: s.bold.filter(|_| patch.bold.is_none()),
            italic: s.italic.filter(|_| patch.italic.is_none()),
            underline: s.underline.filter(|_| patch.underline.is_none()),
            strike: s.strike.filter(|_| patch.strike.is_none()),
            color: s.color.filter(|_| patch.color.is_none()),
            tracking: s.tracking.filter(|_| patch.tracking.is_none()),
        })
        .collect();
    compress(&styles, base)
}

/// Where `new` differs from `old`: the place it starts, how many of
/// `old`'s characters were replaced, and how many of `new`'s took their
/// place — what is in common at both ends left out.
pub fn edit_between(old: &str, new: &str) -> (usize, usize, usize) {
    let (a, b): (Vec<char>, Vec<char>) = (old.chars().collect(), new.chars().collect());
    let front = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let most = a.len().min(b.len()) - front;
    let back = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take(most)
        .take_while(|(x, y)| x == y)
        .count();
    (front, a.len() - front - back, b.len() - front - back)
}

/// The runs after `removed` characters at `at` were replaced by
/// `inserted` new ones, in a text `n` characters long before. What is
/// typed takes the style of the character before it — the first one's,
/// at the very start — with `carry` laid over it: what the bar set for
/// the next letters while nothing was selected.
pub fn edit(
    runs: &[Run],
    n: usize,
    base: &TextStyle,
    (at, removed, inserted): (usize, usize, usize),
    carry: Option<&RunStyle>,
) -> Vec<Run> {
    let mut styles = expand(runs, n);
    let from = if at > 0 { at - 1 } else { at };
    let like = styles.get(from).cloned().unwrap_or_default();
    let like = match carry {
        Some(c) => merge(&like, c),
        None => like,
    };
    let at = at.min(styles.len());
    let end = (at + removed).min(styles.len());
    styles.splice(at..end, std::iter::repeat_n(like, inserted));
    compress(&styles, base)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> TextStyle {
        TextStyle::with_size(10.0, "#000000")
    }

    fn bold() -> RunStyle {
        RunStyle {
            bold: Some(true),
            ..RunStyle::default()
        }
    }

    fn red() -> RunStyle {
        RunStyle {
            color: Some("#ff0000".into()),
            ..RunStyle::default()
        }
    }

    fn run(start: usize, end: usize, style: RunStyle) -> Run {
        Run { start, end, style }
    }

    #[test]
    fn a_run_sets_the_characters_it_spans_and_no_others() {
        let runs = [run(2, 4, bold())];
        assert!(!style_at(&base(), &runs, 1).bold);
        assert!(style_at(&base(), &runs, 2).bold);
        assert!(style_at(&base(), &runs, 3).bold);
        assert!(!style_at(&base(), &runs, 4).bold);
    }

    #[test]
    fn a_change_over_a_stretch_splits_and_joins_runs_as_it_must() {
        // "hello world": bold "llo wo", then red over "o w".
        let runs = apply(&[], 11, &base(), 2, 8, &bold());
        assert_eq!(runs, [run(2, 8, bold())]);
        let runs = apply(&runs, 11, &base(), 4, 7, &red());
        assert_eq!(
            runs,
            [run(2, 4, bold()), run(4, 7, merge(&bold(), &red())), run(7, 8, bold())]
        );
        // Bold over what is left of the word joins it to its neighbour.
        let joined = apply(&[run(0, 2, bold())], 5, &base(), 2, 5, &bold());
        assert_eq!(joined, [run(0, 5, bold())]);
    }

    #[test]
    fn a_run_saying_what_the_text_says_is_no_run() {
        let off = RunStyle {
            bold: Some(false),
            ..RunStyle::default()
        };
        assert!(apply(&[], 5, &base(), 0, 3, &off).is_empty());
        let mut b = base();
        b.bold = true;
        assert_eq!(apply(&[], 5, &b, 1, 3, &off), [run(1, 3, off.clone())], "unless the text says otherwise");
        assert!(tidy(&[run(0, 2, RunStyle::default())], 5, &base()).is_empty());
    }

    #[test]
    fn what_is_typed_takes_the_style_of_the_letter_before_it() {
        let runs = [run(0, 3, bold())];
        // "abc|de": typed after the bold letters, bold too.
        assert_eq!(edit(&runs, 5, &base(), (3, 0, 2), None), [run(0, 5, bold())]);
        // At the very start, the first letter's.
        assert_eq!(edit(&runs, 5, &base(), (0, 0, 1), None), [run(0, 4, bold())]);
        // After plain letters, plain.
        assert_eq!(edit(&runs, 5, &base(), (5, 0, 2), None), [run(0, 3, bold())]);
        // With something set for the next letters, that on top.
        assert_eq!(
            edit(&runs, 5, &base(), (5, 0, 2), Some(&red())),
            [run(0, 3, bold()), run(5, 7, red())]
        );
    }

    #[test]
    fn a_stretch_taken_out_takes_its_runs_with_it() {
        let runs = [run(0, 2, bold()), run(4, 8, red())];
        // "ab" bold, "efgh" red; take out "bcde".
        assert_eq!(edit(&runs, 10, &base(), (1, 4, 0), None), [run(0, 1, bold()), run(1, 4, red())]);
        // Replacing the whole red stretch by one letter: it is typed in
        // the style before it, which is plain.
        assert_eq!(edit(&runs, 10, &base(), (4, 4, 1), None), [run(0, 2, bold())]);
    }

    #[test]
    fn an_edit_is_found_between_two_versions_of_a_text() {
        assert_eq!(edit_between("hello", "hello!"), (5, 0, 1));
        assert_eq!(edit_between("hello", "help"), (3, 2, 1));
        assert_eq!(edit_between("aaa", "aa"), (2, 1, 0));
        assert_eq!(edit_between("ação", "açãoo"), (4, 0, 1), "in characters, not bytes");
        assert_eq!(edit_between("same", "same"), (4, 0, 0));
        assert_eq!(edit_between("", "new"), (0, 0, 3));
    }

    #[test]
    fn a_style_given_to_the_whole_text_leaves_the_runs_for_the_rest() {
        let runs = [run(0, 3, merge(&bold(), &red()))];
        let stripped = strip(&runs, 5, &base(), &bold());
        assert_eq!(stripped, [run(0, 3, red())]);
        let mut after = base();
        after.bold = true;
        after.size = 20.0;
        assert_eq!(
            diff(&base(), &after),
            RunStyle {
                bold: Some(true),
                size: Some(20.0),
                ..RunStyle::default()
            }
        );
        assert!(diff(&base(), &base()).is_empty());
    }
}
