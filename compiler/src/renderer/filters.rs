use serde_json::Value;

use crate::ast::Filter;
use crate::error::{CompileError, CompileResult, ErrorCode};

pub fn apply_filter(value: Value, filter: &Filter, file: &str) -> CompileResult<Value> {
    match filter.name.as_str() {
        "upper" => filter_upper(value),
        "lower" => filter_lower(value),
        "currency" => filter_currency(value, filter, file),
        "truncate" => filter_truncate(value, filter, file),
        "fixed" => filter_fixed(value, filter, file),
        "date" => filter_date(value, filter, file),
        "default" => filter_default(value, filter, file),
        "plural" => filter_plural(value, filter, file),
        "int" => filter_int(value),
        "float" => filter_float(value),
        "js" => filter_js(value),
        "json" => filter_json(value),
        "unsafe_js" => Ok(Value::String(value_to_string(&value))),
        name => Err(CompileError::render(
            file,
            filter.line,
            filter.column,
            format!("Unknown filter: {name}"),
            ErrorCode::UnknownFilter,
        )),
    }
}

pub fn apply_loop_filter(
    mut items: Vec<Value>,
    filter: &Filter,
    file: &str,
) -> CompileResult<Vec<Value>> {
    match filter.name.as_str() {
        "limit" => {
            let n = get_usize_arg(filter, 0, file)?;
            items.truncate(n);
            Ok(items)
        }
        "sort" => {
            let key = get_string_arg(filter, 0, file).ok();
            let dir = get_string_arg(filter, 1, file).ok();
            let descending = dir.as_deref() == Some("desc");

            items.sort_by(|a, b| {
                let va = key.as_ref().and_then(|k| a.get(k)).unwrap_or(a);
                let vb = key.as_ref().and_then(|k| b.get(k)).unwrap_or(b);
                let cmp = value_cmp(va, vb);
                if descending { cmp.reverse() } else { cmp }
            });
            Ok(items)
        }
        "reverse" => {
            items.reverse();
            Ok(items)
        }
        "filter" => {
            let key = get_string_arg(filter, 0, file)?;
            let val = get_filter_value_arg(filter, 1);
            items.retain(|item| {
                item.get(&key)
                    .map(|v| values_equal(v, &val))
                    .unwrap_or(false)
            });
            Ok(items)
        }
        name => Err(CompileError::render(
            file,
            filter.line,
            filter.column,
            format!("Unknown loop filter: {name}"),
            ErrorCode::UnknownFilter,
        )),
    }
}

fn filter_upper(value: Value) -> CompileResult<Value> {
    Ok(Value::String(value_to_string(&value).to_uppercase()))
}

/// `js`: escape a value for safe interpolation inside a JavaScript string
/// literal (see `escape_js_string`). Use on untrusted data bound into a
/// JS-evaluated attribute (`@click`, `x-data`, `:bind`, native `on*`).
fn filter_js(value: Value) -> CompileResult<Value> {
    Ok(Value::String(crate::renderer::escape::escape_js_string(
        &value_to_string(&value),
    )))
}

/// `json`: serialize a value as JSON and make it safe to embed in a `<script>`
/// body (see `escape_script_safe`), for the common
/// `const data = ${ items | json };` shape. Unlike `js` it does NOT quote the
/// result - the JSON literal IS the JS value - so it is the right filter for an
/// object or array and the wrong one for a bare string slot inside quotes.
///
/// `escape_script_safe` only rewrites `<` and U+2028/U+2029, all of which are
/// legal JSON string escapes, so the output is still valid JSON and cannot close
/// the script element.
fn filter_json(value: Value) -> CompileResult<Value> {
    let json = serde_json::to_string(&value).unwrap_or_else(|_| "null".to_string());
    Ok(Value::String(crate::renderer::escape::escape_script_safe(
        &json,
    )))
}

fn filter_lower(value: Value) -> CompileResult<Value> {
    Ok(Value::String(value_to_string(&value).to_lowercase()))
}

fn filter_currency(value: Value, filter: &Filter, file: &str) -> CompileResult<Value> {
    let amount = value_to_f64(&value).unwrap_or(0.0);
    let currency = get_string_arg(filter, 0, file).unwrap_or_else(|_| "USD".to_string());

    // Format as locale currency.  We do not use a full ICU library to keep
    // WASM binary size small; instead we format manually with known symbols.
    let (symbol, decimal_sep, thousands_sep) = match currency.as_str() {
        "USD" => ("$".to_string(), '.', ','),
        "EUR" => ("€".to_string(), ',', '.'),
        "GBP" => ("£".to_string(), '.', ','),
        "JPY" => ("¥".to_string(), '.', ','),
        "CAD" => ("CA$".to_string(), '.', ','),
        "AUD" => ("AU$".to_string(), '.', ','),
        "CHF" => ("CHF ".to_string(), '.', '\''),
        // Unknown codes keep the code as a prefix so the amount is never
        // printed with no currency indication at all.
        _ => (format!("{currency} "), '.', ','),
    };

    let formatted = format_currency(amount, &symbol, decimal_sep, thousands_sep, &currency);
    Ok(Value::String(formatted))
}

fn format_currency(
    amount: f64,
    symbol: &str,
    decimal_sep: char,
    thousands_sep: char,
    currency: &str,
) -> String {
    // JPY has no decimal places
    let decimal_places = if currency == "JPY" { 0 } else { 2 };
    let abs = amount.abs();
    // MONEY SAFETY: a Rust float-to-int cast SATURATES rather than wrapping, so
    // `(1e309 * 100.0).round() as u64` used to yield u64::MAX and render
    // `$184,467,440,737,095,516.15` - a plausible-looking price that is entirely
    // fabricated. In a commerce framework a wrong number that reads as real is
    // worse than a visibly broken one, so a non-finite or unrepresentable amount
    // renders a marker nobody can mistake for money. u128 cents keeps every
    // amount a real price could ever hold well inside the exact branch.
    let scaled = if decimal_places > 0 {
        (abs * 100.0).round()
    } else {
        abs.round()
    };
    if !scaled.is_finite() || scaled > u128::MAX as f64 {
        let marker = if amount.is_nan() { "NaN" } else { "Infinity" };
        let out = format!("{symbol}{marker}");
        return if amount.is_sign_negative() && !amount.is_nan() {
            format!("-{out}")
        } else {
            out
        };
    }
    let scaled = scaled as u128;
    // Round once in the smallest unit so the fraction carries into the integer
    // part (19.999 must format as 20.00, not 19.100).
    let (integer_part, decimal_part) = if decimal_places > 0 {
        (scaled / 100, scaled % 100)
    } else {
        (scaled, 0)
    };

    let int_str = integer_part.to_string();
    let mut int_formatted = String::new();
    for (i, ch) in int_str.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            int_formatted.push(thousands_sep);
        }
        int_formatted.push(ch);
    }
    let int_formatted: String = int_formatted.chars().rev().collect();

    let result = if decimal_places > 0 {
        format!("{symbol}{int_formatted}{decimal_sep}{decimal_part:0>2}")
    } else {
        format!("{symbol}{int_formatted}")
    };

    if amount < 0.0 {
        format!("-{result}")
    } else {
        result
    }
}

fn filter_truncate(value: Value, filter: &Filter, file: &str) -> CompileResult<Value> {
    let n = get_usize_arg(filter, 0, file).unwrap_or(50);
    let s = value_to_string(&value);
    let suffix = get_string_arg(filter, 1, file).unwrap_or_else(|_| "...".to_string());

    if s.chars().count() > n {
        let truncated: String = s.chars().take(n).collect();
        Ok(Value::String(format!(
            "{}{suffix}",
            drop_unterminated_tag(&truncated)
        )))
    } else {
        Ok(Value::String(s))
    }
}

/// Cut back to before a trailing UNTERMINATED `<` so truncation cannot emit half
/// a tag.
///
/// Filters run at RENDER time while `richtext` is sanitized at MERGE time, so a
/// filter always operates on already-sanitized markup. `{html body | truncate(12)}`
/// over `<a href="https://example.com">x</a>` used to emit `<a href="htt...`,
/// an open tag whose attribute then swallows every following byte of the page
/// into itself until the next quote - real mangling, even where it is not
/// provably XSS (the swallowed content is theme-authored).
///
/// Only a `<` with no `>` after it is a problem; a `<` that closed is a whole
/// tag and stays. This intentionally does not try to BALANCE tags - an unclosed
/// `<a>` is inert markup, an unclosed `<a href="` is not.
fn drop_unterminated_tag(s: &str) -> &str {
    match s.rfind('<') {
        Some(i) if !s[i..].contains('>') => &s[..i],
        _ => s,
    }
}

fn filter_fixed(value: Value, filter: &Filter, file: &str) -> CompileResult<Value> {
    // Cap precision. `n` comes from `get_usize_arg`, so it is a LITERAL in the template
    // source and therefore template-author-controlled, not data - `${ x | fixed(n) }` cannot
    // take its precision from a rendered value. So this is a self-inflicted-footgun guard, not
    // a trust boundary: without it `{:.n$}` allocates a string of `n` digits, and a typo of
    // `fixed(1000000)` costs a megabyte per call for no visible reason.
    const MAX_FIXED_PRECISION: usize = 100;
    let n = get_usize_arg(filter, 0, file)
        .unwrap_or(2)
        .min(MAX_FIXED_PRECISION);
    let f = value_to_f64(&value).unwrap_or(0.0);
    Ok(Value::String(format!("{f:.n$}")))
}

fn filter_date(value: Value, filter: &Filter, file: &str) -> CompileResult<Value> {
    let s = value_to_string(&value);
    let format_str = get_string_arg(filter, 0, file).unwrap_or_else(|_| "MMM DD, YYYY".to_string());
    // Accepts ISO date strings: YYYY-MM-DD or YYYY-MM-DDTHH:MM:SSZ
    let formatted = parse_and_format_date(&s, &format_str);
    Ok(Value::String(formatted))
}

fn parse_and_format_date(s: &str, format: &str) -> String {
    let parts: Vec<&str> = s
        .trim()
        .split(['T', ' '])
        .next()
        .unwrap_or("")
        .split('-')
        .collect();
    if parts.len() < 3 {
        return s.to_string();
    }
    let year: u32 = parts[0].parse().unwrap_or(0);
    let month: u32 = parts[1].parse().unwrap_or(0);
    let day: u32 = parts[2].parse().unwrap_or(0);

    let month_short = [
        "", "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let month_long = [
        "",
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];

    let m_short = month_short.get(month as usize).copied().unwrap_or("");
    let m_long = month_long.get(month as usize).copied().unwrap_or("");

    // Tokenize the format string in a single pass (longest token first) so an
    // already-substituted value is never re-scanned: chained global replaces
    // corrupted month names containing M or D ("Dec 15" became "15ec 15").
    let yyyy = year.to_string();
    let yy = format!("{:0>2}", year % 100);
    let mm = format!("{month:0>2}");
    let m = month.to_string();
    let dd = format!("{day:0>2}");
    let d = day.to_string();
    let tokens: [(&str, &str); 8] = [
        ("YYYY", &yyyy),
        ("YY", &yy),
        ("MMMM", m_long),
        ("MMM", m_short),
        ("MM", &mm),
        ("M", &m),
        ("DD", &dd),
        ("D", &d),
    ];

    let mut out = String::with_capacity(format.len() + 8);
    let mut rest = format;
    while !rest.is_empty() {
        if let Some((tok, repl)) = tokens.iter().find(|(t, _)| rest.starts_with(t)) {
            out.push_str(repl);
            rest = &rest[tok.len()..];
        } else {
            let ch = rest.chars().next().unwrap_or_default();
            out.push(ch);
            rest = &rest[ch.len_utf8()..];
        }
    }
    out
}

fn filter_default(value: Value, filter: &Filter, _file: &str) -> CompileResult<Value> {
    let is_empty = match &value {
        Value::Null => true,
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        _ => false,
    };

    if is_empty {
        if filter.args.is_empty() {
            Ok(Value::String(String::new()))
        } else {
            // Return null so the renderer can substitute the evaluated arg via its own
            // `default` filter override path.
            Ok(Value::Null)
        }
    } else {
        Ok(value)
    }
}

fn filter_plural(value: Value, filter: &Filter, file: &str) -> CompileResult<Value> {
    let count = value_to_f64(&value).unwrap_or(0.0);
    let singular = get_string_arg(filter, 0, file).unwrap_or_else(|_| "item".to_string());
    let plural = get_string_arg(filter, 1, file).unwrap_or_else(|_| format!("{singular}s"));

    let word = if (count - 1.0).abs() < f64::EPSILON {
        &singular
    } else {
        &plural
    };
    let n_fmt = if count.fract() == 0.0 {
        format!("{}", count as i64)
    } else {
        format!("{count}")
    };
    Ok(Value::String(format!("{n_fmt} {word}")))
}

fn filter_int(value: Value) -> CompileResult<Value> {
    let n = match &value {
        Value::Number(n) => {
            // as_i64() returns None for float-typed JSON numbers; fall back to f64
            n.as_i64()
                .unwrap_or_else(|| n.as_f64().unwrap_or(0.0) as i64)
        }
        Value::String(s) => s
            .trim()
            .parse::<i64>()
            .unwrap_or_else(|_| s.trim().parse::<f64>().map(|f| f as i64).unwrap_or(0)),
        Value::Bool(true) => 1,
        Value::Bool(false) => 0,
        _ => 0,
    };
    Ok(Value::Number(n.into()))
}

fn filter_float(value: Value) -> CompileResult<Value> {
    let f = value_to_f64(&value).unwrap_or(0.0);
    Ok(Value::Number(
        serde_json::Number::from_f64(f).unwrap_or_else(|| 0.into()),
    ))
}

#[must_use]
pub fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.to_string()
            } else if let Some(f) = n.as_f64() {
                format!("{f}")
            } else {
                n.to_string()
            }
        }
        Value::Bool(b) => b.to_string(),
        Value::Null => String::new(),
        // SECURITY: an Array/Object stringifies to JSON. That JSON may be emitted
        // raw (e.g. `{html data}` into a `<script>`), so make it script-safe: a
        // value containing `</script>` or `<!--` (or U+2028/U+2029) must not be
        // able to terminate the script element / HTML comment. `escape_script_safe`
        // only escapes `<` and the JS line separators, so the output is still
        // valid JSON.
        Value::Array(_) | Value::Object(_) => crate::renderer::escape::escape_script_safe(
            &serde_json::to_string(v).unwrap_or_default(),
        ),
    }
}

fn value_to_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

fn get_string_arg(filter: &Filter, idx: usize, file: &str) -> CompileResult<String> {
    filter
        .args
        .get(idx)
        .ok_or_else(|| {
            CompileError::render(
                file,
                filter.line,
                filter.column,
                format!(
                    "Filter '{}' requires at least {} argument(s)",
                    filter.name,
                    idx + 1
                ),
                ErrorCode::InvalidFilterArgs,
            )
        })
        .map(|expr| {
            // Filter args are evaluated at parse time as literals when possible.
            // The renderer evaluates them properly; here we extract string literals.
            match &expr.kind {
                crate::ast::ExprKind::StringLiteral(s) => s.clone(),
                crate::ast::ExprKind::NumberLiteral(n) => n.to_string(),
                crate::ast::ExprKind::BoolLiteral(b) => b.to_string(),
                _ => String::new(),
            }
        })
}

fn get_usize_arg(filter: &Filter, idx: usize, file: &str) -> CompileResult<usize> {
    let s = get_string_arg(filter, idx, file)?;
    s.parse::<usize>().map_err(|_| {
        CompileError::render(
            file,
            filter.line,
            filter.column,
            format!("Filter '{}' argument {} must be a number", filter.name, idx),
            ErrorCode::InvalidFilterArgs,
        )
    })
}

fn get_filter_value_arg(filter: &Filter, idx: usize) -> Value {
    filter
        .args
        .get(idx)
        .map(|expr| match &expr.kind {
            crate::ast::ExprKind::StringLiteral(s) => Value::String(s.clone()),
            crate::ast::ExprKind::NumberLiteral(n) => {
                Value::Number(serde_json::Number::from_f64(*n).unwrap_or_else(|| 0.into()))
            }
            crate::ast::ExprKind::BoolLiteral(b) => Value::Bool(*b),
            crate::ast::ExprKind::Null => Value::Null,
            _ => Value::Null,
        })
        .unwrap_or(Value::Null)
}

fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::String(s1), Value::String(s2)) => s1 == s2,
        (Value::Bool(b1), Value::Bool(b2)) => b1 == b2,
        (Value::Null, Value::Null) => true,
        (Value::Number(n1), Value::Number(n2)) => n1
            .as_f64()
            .zip(n2.as_f64())
            .map(|(a, b)| (a - b).abs() < f64::EPSILON)
            .unwrap_or(false),
        _ => false,
    }
}

/// Rank a JSON value by TYPE, so that values of different types have a defined order instead of
/// comparing equal. See `value_cmp`.
fn type_rank(v: &Value) -> u8 {
    match v {
        Value::Null => 0,
        Value::Bool(_) => 1,
        Value::Number(_) => 2,
        Value::String(_) => 3,
        Value::Array(_) => 4,
        Value::Object(_) => 5,
    }
}

/// A TOTAL ORDER over JSON values, used by the `sort` loop filter.
///
/// Returning `Ordering::Equal` for every cross-type pair is NOT a total order: given `1`, `"a"`
/// and `2` it makes `1 == "a"` and `"a" == 2` yet `1 < 2`, breaking transitivity. `slice::sort_by`
/// documents that behaviour as UNSPECIFIED for a comparator that is not a total order - it is
/// allowed to return values in any order OR to panic, and the standard library's detection of the
/// violation is best-effort and has changed between releases. So the cross-type case needs a real
/// ordering here: without one, a page built from a mixed array can reshuffle between renders on a
/// toolchain that does not panic, and start panicking on one that does.
///
/// Arrays sort by length then elementwise; objects by length only, because an object has no
/// intrinsic order and inventing one from key iteration would not be stable.
fn value_cmp(a: &Value, b: &Value) -> std::cmp::Ordering {
    value_cmp_at(a, b, 0)
}

/// Depth beyond which nested arrays compare by length alone.
///
/// The comparator is called O(n log n) times by `sort_by`, on values that came from
/// untrusted JSON, so recursing once per nesting level needs an explicit bound rather
/// than a borrowed one. Truncating to a length comparison keeps the result a valid total order
/// (everything at the cutoff simply ties), which is the property `sort_by` requires.
const VALUE_CMP_MAX_DEPTH: u32 = 32;

fn value_cmp_at(a: &Value, b: &Value, depth: u32) -> std::cmp::Ordering {
    match (a, b) {
        (Value::String(s1), Value::String(s2)) => s1.cmp(s2),
        (Value::Number(n1), Value::Number(n2)) => {
            let f1 = n1.as_f64().unwrap_or(0.0);
            let f2 = n2.as_f64().unwrap_or(0.0);
            // NaN cannot survive `serde_json`, but `as_f64` can also fail on a u64 too large
            // for f64. Falling back to `Equal` is safe HERE because both sides are Numbers, so
            // it only ever collapses two same-type values and transitivity is preserved.
            f1.partial_cmp(&f2).unwrap_or(std::cmp::Ordering::Equal)
        }
        (Value::Bool(b1), Value::Bool(b2)) => b1.cmp(b2),
        (Value::Null, Value::Null) => std::cmp::Ordering::Equal,
        (Value::Array(a1), Value::Array(a2)) => {
            let by_len = a1.len().cmp(&a2.len());
            if by_len.is_ne() || depth >= VALUE_CMP_MAX_DEPTH {
                return by_len;
            }
            a1.iter()
                .zip(a2.iter())
                .map(|(x, y)| value_cmp_at(x, y, depth + 1))
                .find(|o| o.is_ne())
                .unwrap_or(std::cmp::Ordering::Equal)
        }
        // An object has no intrinsic order, and deriving one from key iteration would not be
        // stable across builds, so objects tie unless their sizes differ.
        (Value::Object(o1), Value::Object(o2)) => o1.len().cmp(&o2.len()),
        // Different types: ordered by type rank. This is the branch that makes the whole
        // function a total order.
        _ => type_rank(a).cmp(&type_rank(b)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{ExprKind, Expression, Filter};
    use serde_json::json;

    fn make_filter(name: &str, args: Vec<Expression>) -> Filter {
        Filter {
            name: name.to_string(),
            args,
            line: 1,
            column: 1,
        }
    }

    fn str_expr(s: &str) -> Expression {
        Expression::new(ExprKind::StringLiteral(s.to_string()), 1, 1)
    }

    fn num_expr(n: f64) -> Expression {
        Expression::new(ExprKind::NumberLiteral(n), 1, 1)
    }

    #[test]
    fn test_upper() {
        let f = make_filter("upper", vec![]);
        let result = apply_filter(json!("hello"), &f, "test").unwrap();
        assert_eq!(result, json!("HELLO"));
    }

    #[test]
    fn test_lower() {
        let f = make_filter("lower", vec![]);
        let result = apply_filter(json!("WORLD"), &f, "test").unwrap();
        assert_eq!(result, json!("world"));
    }

    #[test]
    fn test_truncate() {
        let f = make_filter("truncate", vec![num_expr(5.0)]);
        let result = apply_filter(json!("hello world"), &f, "test").unwrap();
        assert_eq!(result, json!("hello..."));
    }

    #[test]
    fn test_truncate_short_string() {
        let f = make_filter("truncate", vec![num_expr(100.0)]);
        let result = apply_filter(json!("hi"), &f, "test").unwrap();
        assert_eq!(result, json!("hi"));
    }

    #[test]
    fn test_truncate_empty() {
        let f = make_filter("truncate", vec![num_expr(10.0)]);
        let result = apply_filter(json!(""), &f, "test").unwrap();
        assert_eq!(result, json!(""));
    }

    #[test]
    fn test_fixed() {
        let f = make_filter("fixed", vec![num_expr(2.0)]);
        let result = apply_filter(json!(1.23456), &f, "test").unwrap();
        assert_eq!(result, json!("1.23"));
    }

    #[test]
    fn test_fixed_zero() {
        let f = make_filter("fixed", vec![num_expr(2.0)]);
        let result = apply_filter(json!(0), &f, "test").unwrap();
        assert_eq!(result, json!("0.00"));
    }

    #[test]
    fn test_currency_usd() {
        let f = make_filter("currency", vec![str_expr("USD")]);
        let result = apply_filter(json!(1234.56), &f, "test").unwrap();
        assert_eq!(result, json!("$1,234.56"));
    }

    #[test]
    fn test_currency_default_usd() {
        let f = make_filter("currency", vec![]);
        let result = apply_filter(json!(100.0), &f, "test").unwrap();
        let s = result.as_str().unwrap();
        assert!(s.contains('$'));
    }

    #[test]
    fn test_currency_eur() {
        let f = make_filter("currency", vec![str_expr("EUR")]);
        let result = apply_filter(json!(1000.0), &f, "test").unwrap();
        let s = result.as_str().unwrap();
        assert!(s.contains('€'));
    }

    #[test]
    fn test_currency_fraction_carries_into_integer() {
        let f = make_filter("currency", vec![str_expr("USD")]);
        let result = apply_filter(json!(19.999), &f, "test").unwrap();
        assert_eq!(result, json!("$20.00"));
        let result = apply_filter(json!(1.999), &f, "test").unwrap();
        assert_eq!(result, json!("$2.00"));
    }

    #[test]
    fn test_currency_unknown_code_prefix() {
        let f = make_filter("currency", vec![str_expr("SEK")]);
        let result = apply_filter(json!(10.0), &f, "test").unwrap();
        assert_eq!(result, json!("SEK 10.00"));
    }

    #[test]
    fn test_currency_jpy_rounds() {
        let f = make_filter("currency", vec![str_expr("JPY")]);
        let result = apply_filter(json!(999.6), &f, "test").unwrap();
        assert_eq!(result, json!("¥1,000"));
    }

    #[test]
    fn test_currency_negative() {
        let f = make_filter("currency", vec![str_expr("USD")]);
        let result = apply_filter(json!(-50.0), &f, "test").unwrap();
        let s = result.as_str().unwrap();
        assert!(s.starts_with('-'));
    }

    #[test]
    fn test_date_default_format() {
        let f = make_filter("date", vec![]);
        let result = apply_filter(json!("2024-01-15"), &f, "test").unwrap();
        assert_eq!(result, json!("Jan 15, 2024"));
    }

    #[test]
    fn test_date_custom_format() {
        let f = make_filter("date", vec![str_expr("MM/DD/YYYY")]);
        let result = apply_filter(json!("2024-03-05"), &f, "test").unwrap();
        assert_eq!(result, json!("03/05/2024"));
    }

    #[test]
    fn test_date_december_default_format() {
        let f = make_filter("date", vec![]);
        let result = apply_filter(json!("2024-12-15"), &f, "test").unwrap();
        assert_eq!(result, json!("Dec 15, 2024"));
    }

    #[test]
    fn test_date_long_month_names_not_corrupted() {
        let f = make_filter("date", vec![str_expr("MMMM D, YYYY")]);
        let result = apply_filter(json!("2024-05-03"), &f, "test").unwrap();
        assert_eq!(result, json!("May 3, 2024"));
        let result = apply_filter(json!("2024-03-09"), &f, "test").unwrap();
        assert_eq!(result, json!("March 9, 2024"));
        let result = apply_filter(json!("2024-12-01"), &f, "test").unwrap();
        assert_eq!(result, json!("December 1, 2024"));
    }

    #[test]
    fn test_default_null() {
        let f = make_filter("default", vec![str_expr("N/A")]);
        let result = apply_filter(json!(null), &f, "test").unwrap();
        // null → returns null (renderer fills in the default arg)
        // This is acceptable; the renderer handles default with evaluated args.
        assert!(result.is_null() || result.as_str() == Some("N/A") || result.as_str() == Some(""));
    }

    #[test]
    fn test_default_empty_string() {
        let f = make_filter("default", vec![str_expr("fallback")]);
        let result = apply_filter(json!(""), &f, "test").unwrap();
        assert!(result.is_null() || result.as_str() == Some(""));
    }

    #[test]
    fn test_default_non_empty_passthrough() {
        let f = make_filter("default", vec![str_expr("fallback")]);
        let result = apply_filter(json!("hello"), &f, "test").unwrap();
        assert_eq!(result, json!("hello"));
    }

    #[test]
    fn test_plural_singular() {
        let f = make_filter("plural", vec![str_expr("item"), str_expr("items")]);
        let result = apply_filter(json!(1), &f, "test").unwrap();
        assert_eq!(result.as_str().unwrap(), "1 item");
    }

    #[test]
    fn test_plural_multiple() {
        let f = make_filter("plural", vec![str_expr("item"), str_expr("items")]);
        let result = apply_filter(json!(5), &f, "test").unwrap();
        assert_eq!(result.as_str().unwrap(), "5 items");
    }

    #[test]
    fn test_plural_zero() {
        let f = make_filter("plural", vec![str_expr("item"), str_expr("items")]);
        let result = apply_filter(json!(0), &f, "test").unwrap();
        assert_eq!(result.as_str().unwrap(), "0 items");
    }

    #[test]
    fn test_int_from_string() {
        let f = make_filter("int", vec![]);
        let result = apply_filter(json!("42"), &f, "test").unwrap();
        assert_eq!(result, json!(42));
    }

    #[test]
    fn test_int_from_float() {
        let f = make_filter("int", vec![]);
        let result = apply_filter(json!(3.7), &f, "test").unwrap();
        assert_eq!(result.as_i64().unwrap(), 3);
    }

    #[test]
    fn test_int_null() {
        let f = make_filter("int", vec![]);
        let result = apply_filter(json!(null), &f, "test").unwrap();
        assert_eq!(result, json!(0));
    }

    #[test]
    fn test_float_from_string() {
        let f = make_filter("float", vec![]);
        let result = apply_filter(json!("1.25"), &f, "test").unwrap();
        let v = result.as_f64().unwrap();
        assert!((v - 1.25).abs() < 0.001);
    }

    #[test]
    fn test_float_null() {
        let f = make_filter("float", vec![]);
        let result = apply_filter(json!(null), &f, "test").unwrap();
        assert_eq!(result.as_f64().unwrap(), 0.0);
    }

    #[test]
    fn test_unknown_filter_error() {
        let f = make_filter("nonexistent", vec![]);
        let result = apply_filter(json!("x"), &f, "test");
        assert!(result.is_err());
    }

    // Loop filter tests
    #[test]
    fn test_loop_limit() {
        let f = make_filter("limit", vec![num_expr(3.0)]);
        let items = vec![json!(1), json!(2), json!(3), json!(4), json!(5)];
        let result = apply_loop_filter(items, &f, "test").unwrap();
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_loop_limit_zero() {
        let f = make_filter("limit", vec![num_expr(0.0)]);
        let items = vec![json!(1), json!(2)];
        let result = apply_loop_filter(items, &f, "test").unwrap();
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_loop_reverse() {
        let f = make_filter("reverse", vec![]);
        let items = vec![json!(1), json!(2), json!(3)];
        let result = apply_loop_filter(items, &f, "test").unwrap();
        assert_eq!(result, vec![json!(3), json!(2), json!(1)]);
    }

    #[test]
    fn test_loop_sort_by_key() {
        let f = make_filter("sort", vec![str_expr("name")]);
        let items = vec![
            json!({"name": "Banana"}),
            json!({"name": "Apple"}),
            json!({"name": "Cherry"}),
        ];
        let result = apply_loop_filter(items, &f, "test").unwrap();
        assert_eq!(result[0]["name"], json!("Apple"));
        assert_eq!(result[2]["name"], json!("Cherry"));
    }

    #[test]
    fn test_loop_sort_desc() {
        let f = make_filter("sort", vec![str_expr("price"), str_expr("desc")]);
        let items = vec![
            json!({"price": 10}),
            json!({"price": 30}),
            json!({"price": 20}),
        ];
        let result = apply_loop_filter(items, &f, "test").unwrap();
        assert_eq!(result[0]["price"], json!(30));
    }

    #[test]
    fn test_loop_filter_by_key() {
        let f = make_filter("filter", vec![str_expr("status"), str_expr("active")]);
        let items = vec![
            json!({"status": "active", "id": 1}),
            json!({"status": "inactive", "id": 2}),
            json!({"status": "active", "id": 3}),
        ];
        let result = apply_loop_filter(items, &f, "test").unwrap();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_loop_filter_empty_result() {
        let f = make_filter("filter", vec![str_expr("x"), str_expr("y")]);
        let items = vec![json!({"x": "a"})];
        let result = apply_loop_filter(items, &f, "test").unwrap();
        assert!(result.is_empty());
    }
}
