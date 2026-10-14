use alloc::string::String;
use core::fmt::Write;

/// A value substituted into a `{placeholder}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arg<'a> {
    Str(&'a str),
    Int(i64),
}

/// Replaces `{name}` placeholders. `{{` and `}}` produce literal braces;
/// unknown placeholders are kept as written so mistakes stay visible.
pub(crate) fn interpolate(message: &str, args: &[(&str, Arg<'_>)]) -> String {
    interpolate_impl(message, None, args)
}

/// Like [`interpolate`], with `{count}` bound to `count`.
pub(crate) fn interpolate_with_count(
    message: &str,
    count: i64,
    args: &[(&str, Arg<'_>)],
) -> String {
    interpolate_impl(message, Some(count), args)
}

fn interpolate_impl(message: &str, count: Option<i64>, args: &[(&str, Arg<'_>)]) -> String {
    let mut out = String::with_capacity(message.len() + 16);
    let mut rest = message;
    while let Some(open) = rest.find(['{', '}']) {
        out.push_str(&rest[..open]);
        let tail = &rest[open..];
        if let Some(after) = tail.strip_prefix("{{") {
            out.push('{');
            rest = after;
            continue;
        }
        if let Some(after) = tail.strip_prefix("}}") {
            out.push('}');
            rest = after;
            continue;
        }
        if tail.starts_with('{')
            && let Some(close) = tail.find('}')
        {
            let name = &tail[1..close];
            let value = args
                .iter()
                .find(|(arg_name, _)| *arg_name == name)
                .map(|(_, value)| *value)
                .or_else(|| (name == "count").then_some(count).flatten().map(Arg::Int));
            match value {
                Some(Arg::Str(s)) => out.push_str(s),
                Some(Arg::Int(i)) => {
                    let _ = write!(out, "{i}");
                }
                None => out.push_str(&tail[..=close]),
            }
            rest = &tail[close + 1..];
            continue;
        }
        // A lone brace: keep it.
        out.push_str(&tail[..1]);
        rest = &tail[1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_placeholders() {
        let args = [("name", Arg::Str("Ada")), ("n", Arg::Int(-3))];
        assert_eq!(interpolate("{name} has {n}", &args), "Ada has -3");
    }

    #[test]
    fn keeps_unknown_placeholders_and_lone_braces() {
        assert_eq!(interpolate("{missing} } {", &[]), "{missing} } {");
    }

    #[test]
    fn escaped_braces() {
        assert_eq!(interpolate("{{literal}}", &[]), "{literal}");
    }

    #[test]
    fn count_is_implicit() {
        assert_eq!(interpolate_with_count("{count} books", 4, &[]), "4 books");
    }
}
