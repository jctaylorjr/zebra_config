// what's typed in a printer search box: an IP address or hostname, used as it is, or a control
// number, the number on a printer's asset label. ServiceNow names each printer's record by its
// control number and has its serial. a new printer is named CSC and its serial, older ones CSC
// and the control number, sometimes with a dash, and Link-OS 7.6 names one its model and serial
// until it's given a name. so a control number is looked up in ServiceNow, and the printer is
// the one that answers to one of those names and has that serial
use crate::{
    printer::{self, Identity},
    servicenow::{Record, ServiceNow},
};
use futures_util::{StreamExt, future::join_all, stream::FuturesUnordered};
use serde::Serialize;
use std::path::Path;

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Found {
    pub address: String,      // what to reach it by
    pub note: Option<String>, // how a control number led to it
}

impl Found {
    fn plain(address: &str) -> Found {
        Found {
            address: address.to_string(),
            note: None,
        }
    }
}

// a control number: only digits. never looked up as a name, as Windows reads digits alone as an
// IP address
pub fn control_number(q: &str) -> Option<&str> {
    ((3..=12).contains(&q.len()) && q.bytes().all(|b| b.is_ascii_digit())).then_some(q)
}

// the control number a name like CSC722858 or csc-722858 is made of
fn named_for(q: &str) -> Option<&str> {
    let rest = q.get(3..).filter(|_| q[..3].eq_ignore_ascii_case("csc"))?;
    control_number(rest.strip_prefix('-').unwrap_or(rest))
}

// the names a printer with control number `n` may answer to, in capitals
fn names_for(n: &str) -> Vec<String> {
    vec![format!("CSC{n}"), format!("CSC-{n}")]
}

// the names the printer `r` may answer to: CSC and its serial, then its model and serial, as
// Link-OS 7.6 names it, then the control number's
fn names_with_serial(r: &Record, n: &str) -> Vec<String> {
    let serial = r.serial.to_uppercase();
    let mut names = vec![format!("CSC{serial}")];
    if let Some(code) = model_code(&r.model) {
        names.push(format!("{code}-{serial}"));
    }
    names.extend(names_for(n));
    names
}

// a Zebra model's short name, like ZD611 from "Zebra ZD611D-HC Desktop Direct Thermal Printer"
fn model_code(model: &str) -> Option<String> {
    model.split_whitespace().find_map(|word| {
        let b = word.as_bytes();
        (b.len() >= 5
            && b[0].eq_ignore_ascii_case(&b'Z')
            && b[1].is_ascii_alphabetic()
            && b[2..5].iter().all(u8::is_ascii_digit))
        .then(|| word[..5].to_uppercase())
    })
}

pub async fn find(
    sn: &ServiceNow,
    profile: &Path,
    query: &str,
    report: &(dyn Fn(&str) + Sync),
) -> Result<Found, String> {
    let q = query.trim();
    if q.is_empty() {
        return Err("enter an IP address, hostname or control number".into());
    }
    if let Some(n) = control_number(q) {
        return by_number(sn, profile, n, report).await;
    }
    // a name made of a control number is used as it is when a printer answers to it, and
    // otherwise looked up by the number
    if let Some(n) = named_for(q) {
        report(&format!("Looking for {q}"));
        if first(&[q.to_string()], |_| true).await.is_some() {
            return Ok(Found::plain(q));
        }
        return by_number(sn, profile, n, report)
            .await
            .map_err(|e| format!("no printer answered as {q}, and {e}"));
    }
    Ok(Found::plain(q))
}

async fn by_number(
    sn: &ServiceNow,
    profile: &Path,
    n: &str,
    report: &(dyn Fn(&str) + Sync),
) -> Result<Found, String> {
    report(&format!("Looking up {n} in ServiceNow"));
    let records = match sn.printers(profile, n, report).await {
        Ok(r) => r,
        Err(e) => {
            // without ServiceNow, only a printer that calls itself one of the names
            let names = names_for(n);
            report(&format!("Looking for {}", or_list(&names)));
            return match calls_itself(&names).await {
                Some(name) => Ok(Found {
                    note: Some(format!(
                        "{n} is {name}, by its name alone, as ServiceNow couldn't be asked: {e:#}"
                    )),
                    address: name,
                }),
                None => Err(format!("couldn't ask ServiceNow about {n}: {e:#}")),
            };
        }
    };
    // ServiceNow matches names without regard to case and the like, so only its exact ones
    let mut own: Vec<&Record> = records.iter().filter(|r| r.name == n).collect();
    own.sort_by_key(|r| r.serial.is_empty());
    let place = own
        .iter()
        .map(|r| r.place())
        .find(|p| !p.is_empty())
        .map(|p| format!(" at {p}"))
        .unwrap_or_default();
    let (mut serials, mut tried) = (vec![], vec![]);
    for r in own.iter().filter(|r| !r.serial.is_empty()) {
        let names = names_with_serial(r, n);
        report(&format!(
            "Looking for {} with serial {}",
            or_list(&names),
            r.serial
        ));
        let has_serial = |id: &Identity| {
            id.serial
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case(&r.serial))
        };
        if let Some(i) = first(&names, has_serial).await {
            return Ok(Found {
                note: Some(format!("{n} is {}{place}", names[i])),
                address: names[i].clone(),
            });
        }
        serials.push(r.serial.clone());
        for name in names {
            if !tried.contains(&name) {
                tried.push(name);
            }
        }
    }
    if !serials.is_empty() {
        return Err(format!(
            "ServiceNow has {n}{place} as serial {}, but it didn't answer as {}",
            or_list(&serials),
            or_list(&tried)
        ));
    }
    // ServiceNow doesn't have its serial: a printer that calls itself one of the names
    let names = names_for(n);
    report(&format!("Looking for {}", or_list(&names)));
    match calls_itself(&names).await {
        Some(name) => Ok(Found {
            note: Some(format!("{n} is {name}{place}")),
            address: name,
        }),
        None if own.is_empty() => Err(format!(
            "ServiceNow has no printer with control number {n}, and none answered as {}",
            or_list(&names)
        )),
        None => Err(format!(
            "ServiceNow has no serial for {n}{place}, and no printer answered as {}",
            or_list(&names)
        )),
    }
}

// the first of `names` a printer answers to with that name for itself
async fn calls_itself(names: &[String]) -> Option<String> {
    let i = first_each(names, |i, id| {
        id.hostname
            .as_deref()
            .and_then(|h| h.split('.').next())
            .is_some_and(|h| h.eq_ignore_ascii_case(&names[i]))
    })
    .await?;
    Some(names[i].clone())
}

// the first of `names`, in their order, that a printer answers to that `fits`
async fn first(names: &[String], fits: impl Fn(&Identity) -> bool) -> Option<usize> {
    first_each(names, |_, id| fits(id)).await
}

// the same, with each name's own test. the names are all looked up at once, and the answer
// comes as soon as a name fits and every one before it is known not to, so a printer under
// its usual name isn't kept waiting for names that don't exist, which take seconds to rule out.
// Windows is made to look each name up again rather than trust a "no such name" it remembers
async fn first_each(names: &[String], fits: impl Fn(usize, &Identity) -> bool) -> Option<usize> {
    let mut pending: FuturesUnordered<_> = names
        .iter()
        .enumerate()
        .map(|(i, name)| async move {
            let ips: Vec<String> = printer::lookup(name)
                .await
                .iter()
                .map(|ip| ip.to_string())
                .collect();
            let ids = join_all(ips.iter().map(|ip| printer::identify(ip))).await;
            (
                i,
                ids.into_iter().filter_map(Result::ok).collect::<Vec<_>>(),
            )
        })
        .collect();
    let mut known: Vec<Option<bool>> = vec![None; names.len()];
    while let Some((i, ids)) = pending.next().await {
        known[i] = Some(ids.iter().any(|id| fits(i, id)));
        for (j, k) in known.iter().enumerate() {
            match k {
                Some(true) => return Some(j),
                Some(false) => continue,
                None => break,
            }
        }
    }
    None
}

// "A, B or C"
fn or_list(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_a_control_number() {
        assert_eq!(control_number("722858"), Some("722858"));
        assert_eq!(control_number("12"), None);
        assert_eq!(control_number("10.1.2.3"), None);
        assert_eq!(control_number("CSC722858"), None);
        assert_eq!(named_for("CSC722858"), Some("722858"));
        assert_eq!(named_for("csc-722858"), Some("722858"));
        assert_eq!(named_for("Csc722858"), Some("722858"));
        assert_eq!(named_for("CSCDGN260506471"), None);
        assert_eq!(named_for("csc"), None);
        assert_eq!(named_for("é"), None);
    }

    #[test]
    fn names_a_printer_in_capitals() {
        let r = Record {
            name: "722858".into(),
            serial: "dgn260506471".into(),
            model: "Zebra ZD611D-HC Desktop Direct Thermal Printer - Monochrome".into(),
            ..Default::default()
        };
        assert_eq!(
            names_with_serial(&r, "722858"),
            [
                "CSCDGN260506471",
                "ZD611-DGN260506471",
                "CSC722858",
                "CSC-722858"
            ]
        );
        let unknown = Record {
            model: "Zebra Printer".into(),
            ..r.clone()
        };
        assert_eq!(names_with_serial(&unknown, "722858").len(), 3);
        assert_eq!(model_code("Zebra ZQ630 Plus"), Some("ZQ630".into()));
    }

    #[test]
    fn lists_names() {
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(or_list(&names(&["A"])), "A");
        assert_eq!(or_list(&names(&["A", "B", "C"])), "A, B or C");
    }

    #[tokio::test]
    async fn takes_an_address_as_it_is() {
        let sn = ServiceNow::default();
        let found = find(&sn, Path::new("unused"), " 192.0.2.10 ", &|_| {}).await;
        assert_eq!(found, Ok(Found::plain("192.0.2.10")));
        assert!(find(&sn, Path::new("unused"), "  ", &|_| {}).await.is_err());
    }

    // ZEBRA_FIND is what's typed, like a control number; asks ServiceNow as the person signed in
    // to the app's browser
    #[tokio::test]
    #[ignore]
    async fn find_printer() {
        let q = std::env::var("ZEBRA_FIND").expect("set ZEBRA_FIND");
        let profile = Path::new(&std::env::var("APPDATA").unwrap())
            .join("com.jt1280.zebra-configure")
            .join("browser-edge");
        let began = std::time::Instant::now();
        let report = |s: &str| println!("... {:.1}s {s}", began.elapsed().as_secs_f32());
        println!(
            "{:?}",
            find(&ServiceNow::default(), &profile, &q, &report).await
        );
    }
}
