//! The catalog search query language, evaluated against `shared.sqlite`.
//!
//! A query is whitespace-separated terms, all of which must hold:
//!
//! * a bare word or `"quoted phrase"` — full-text match (prefix on the last
//!   word of a bare word) over name, subtitle, rules text and traits, through
//!   the `card_search` FTS5 index the catalog apply job builds;
//! * `key:value` — `title`, `subtitle`, `type`, `aspect`, `arena`, `trait`,
//!   `rarity`, `set`, `variant`, `finish` (short forms `t`, `a`, `r`, `s`, `v`,
//!   `f`); equality, case-insensitive; `title`/`subtitle` are full-text on that
//!   column;
//! * `cost`, `power`, `hp` (short `c`) with `:`/`=`, `<`, `<=`, `>`, `>=`, `!=`
//!   and an integer;
//! * a leading `-` negates a term.
//!
//! The result is printings, in a fixed order, from the shared catalog only.
//! Nothing here takes a tenant: the same query returns identical rows whoever
//! asks, and the index is written only by the apply job (`Catalog::open_writer`).

use rusqlite::params_from_iter;
use rusqlite::types::Value;

use crate::Catalog;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum QueryError {
    #[error("unknown field {0:?}")]
    UnknownField(String),
    #[error("{0:?} needs an integer, got {1:?}")]
    NotANumber(String, String),
    #[error("{0:?} takes only : or = ")]
    BadOperator(String),
    #[error("unterminated quote")]
    Unterminated,
    #[error("empty value for {0:?}")]
    EmptyValue(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub printing_id: String,
    pub card_id: String,
    pub name: String,
    pub subtitle: Option<String>,
    pub set_code: String,
    pub number: String,
    pub variant: String,
    pub finish: String,
    pub rarity: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error(transparent)]
    Query(#[from] QueryError),
    #[error(transparent)]
    Catalog(#[from] crate::CatalogError),
}

struct Compiled {
    wheres: Vec<String>,
    args: Vec<Value>,
}

/// Split into raw tokens, honouring quotes (`title:"Han Solo"`, `-"a b"`).
fn tokens(q: &str) -> Result<Vec<String>, QueryError> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut quoted = false;
    let mut any = false;
    for ch in q.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
                any = true;
                cur.push(ch);
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                any = true;
                cur.push(c);
            }
        }
    }
    if quoted {
        return Err(QueryError::Unterminated);
    }
    if any {
        out.push(cur);
    }
    Ok(out)
}

fn unquote(v: &str) -> (String, bool) {
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        (v[1..v.len() - 1].to_string(), true)
    } else {
        (v.replace('"', ""), false)
    }
}

/// An FTS5 expression for `text`, optionally restricted to a column. Every
/// word is quoted, so user text can never be read as FTS syntax.
fn fts(column: Option<&str>, text: &str, phrase: bool) -> Option<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return None;
    }
    let body = if phrase {
        format!("\"{}\"", words.join(" "))
    } else {
        let last = words.len() - 1;
        words
            .iter()
            .enumerate()
            .map(|(i, w)| format!("\"{w}\"{}", if i == last { "*" } else { "" }))
            .collect::<Vec<_>>()
            .join(" ")
    };
    Some(match column {
        Some(c) => format!("{c} : ({body})"),
        None => body,
    })
}

fn split_term(t: &str) -> (&str, Option<(&str, &str)>) {
    // returns (field-or-whole, Some((op, value)))
    let bytes: Vec<(usize, char)> = t.char_indices().collect();
    for (i, c) in bytes {
        if c == '"' {
            break;
        }
        if matches!(c, ':' | '=' | '<' | '>' | '!') {
            let rest = &t[i..];
            let oplen =
                if rest.starts_with("<=") || rest.starts_with(">=") || rest.starts_with("!=") {
                    2
                } else {
                    1
                };
            return (&t[..i], Some((&rest[..oplen], &rest[oplen..])));
        }
    }
    (t, None)
}

fn compile(q: &str) -> Result<Compiled, QueryError> {
    let mut c = Compiled {
        wheres: vec![],
        args: vec![],
    };
    for raw in tokens(q)? {
        let (neg, term) = match raw.strip_prefix('-') {
            Some(r) if !r.is_empty() => (true, r),
            _ => (false, raw.as_str()),
        };
        let (field, rest) = split_term(term);
        let clause = match rest {
            Some((op, value)) if !field.is_empty() => {
                field_clause(&mut c.args, &field.to_lowercase(), op, value)?
            }
            _ => {
                let (text, phrase) = unquote(term);
                match fts(None, &text, phrase) {
                    Some(m) => {
                        c.args.push(Value::Text(m));
                        "p.card_id IN (SELECT card_id FROM card_search WHERE card_search MATCH ?)"
                            .to_string()
                    }
                    None => continue,
                }
            }
        };
        c.wheres.push(if neg {
            format!("NOT ({clause})")
        } else {
            clause
        });
    }
    Ok(c)
}

fn field_clause(
    args: &mut Vec<Value>,
    field: &str,
    op: &str,
    value: &str,
) -> Result<String, QueryError> {
    let (text, phrase) = unquote(value);
    if text.trim().is_empty() {
        return Err(QueryError::EmptyValue(field.into()));
    }
    let eq_only = |args: &mut Vec<Value>, sql: &str| {
        if op != ":" && op != "=" {
            return Err(QueryError::BadOperator(field.into()));
        }
        args.push(Value::Text(text.clone()));
        Ok(sql.to_string())
    };
    match field {
        "title" | "name" | "subtitle" | "sub" => {
            if op != ":" && op != "=" {
                return Err(QueryError::BadOperator(field.into()));
            }
            let col = if field.starts_with("sub") {
                "subtitle"
            } else {
                "name"
            };
            args.push(Value::Text(fts(Some(col), &text, phrase).unwrap()));
            Ok("p.card_id IN (SELECT card_id FROM card_search WHERE card_search MATCH ?)".into())
        }
        "t" | "type" => eq_only(args, "c.card_type = ? COLLATE NOCASE"),
        "a" | "aspect" => eq_only(
            args,
            "EXISTS (SELECT 1 FROM json_each(c.aspects) WHERE value = ? COLLATE NOCASE)",
        ),
        "arena" => eq_only(
            args,
            "EXISTS (SELECT 1 FROM json_each(c.arenas) WHERE value = ? COLLATE NOCASE)",
        ),
        "trait" => eq_only(
            args,
            "EXISTS (SELECT 1 FROM json_each(c.traits) WHERE value = ? COLLATE NOCASE)",
        ),
        "r" | "rarity" => eq_only(args, "p.rarity = ? COLLATE NOCASE"),
        "s" | "set" => eq_only(args, "p.set_code = ? COLLATE NOCASE"),
        "v" | "variant" => eq_only(args, "p.variant = ? COLLATE NOCASE"),
        "f" | "finish" => eq_only(args, "p.finish = ? COLLATE NOCASE"),
        "c" | "cost" | "power" | "hp" => {
            let col = match field {
                "c" | "cost" => "cost",
                f => f,
            };
            let n: i64 = text
                .trim()
                .parse()
                .map_err(|_| QueryError::NotANumber(field.into(), text.clone()))?;
            let sql_op = match op {
                ":" | "=" => "=",
                o => o,
            };
            args.push(Value::Integer(n));
            Ok(format!("c.{col} {sql_op} ?"))
        }
        other => Err(QueryError::UnknownField(other.into())),
    }
}

/// Run `query` against the catalog. Order is fixed: set order, collector
/// number, printing id.
pub fn search(catalog: &Catalog, query: &str) -> Result<Vec<Hit>, SearchError> {
    let c = compile(query)?;
    let mut sql = String::from(
        "SELECT p.printing_id, p.card_id, c.name, c.subtitle, p.set_code, p.number,
                p.variant, p.finish, p.rarity
         FROM printings p
         JOIN cards c ON c.card_id = p.card_id
         JOIN sets s ON s.set_code = p.set_code",
    );
    if !c.wheres.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&c.wheres.join(" AND "));
    }
    sql.push_str(
        " ORDER BY s.sort_order, p.set_code, CAST(p.number AS INTEGER), p.number, p.printing_id",
    );
    let conn = catalog.conn();
    let mut stmt = conn.prepare(&sql).map_err(crate::CatalogError::from)?;
    let rows = stmt
        .query_map(params_from_iter(c.args.iter()), |r| {
            Ok(Hit {
                printing_id: r.get(0)?,
                card_id: r.get(1)?,
                name: r.get(2)?,
                subtitle: r.get(3)?,
                set_code: r.get(4)?,
                number: r.get(5)?,
                variant: r.get(6)?,
                finish: r.get(7)?,
                rarity: r.get(8)?,
            })
        })
        .map_err(crate::CatalogError::from)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(crate::CatalogError::from)?;
    Ok(rows)
}
