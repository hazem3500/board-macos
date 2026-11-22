//! BOARD_DEMO=1: made-up cards for the CI screenshots on a macOS runner (no network, no Keychain, no pairing).
use crate::logic::{Item, Needs, Work};

fn item(id: &str, title: &str, text: &str, asker: &str, project: &str, kind: &str, options: &[&str], recommend: Option<&str>) -> Item {
    Item {
        id: id.into(),
        title: title.into(),
        text: Some(text.into()),
        asker: Some(asker.into()),
        project: Some(project.into()),
        kind: kind.into(),
        options: if options.is_empty() { None } else { Some(options.iter().map(|o| o.to_string()).collect()) },
        recommend: recommend.map(String::from),
    }
}

pub fn needs() -> Needs {
    let items = vec![
        item(
            "demo:1",
            "Merge the Board for Mac release PR?",
            "CI is green and the security review approved the pairing handoff. Merging ships v1.0 to the releases repo.",
            "HackMaster",
            "oz-home",
            "choice",
            &["Not yet", "Merge now"],
            Some("Merge now"),
        ),
        item(
            "demo:2",
            "What should the Quick add shortcut be?",
            "⌥⌘B is free on your Mac today. Reply with another combination if you'd rather.",
            "Deploy-O-Bot",
            "oz-home",
            "text",
            &[],
            None,
        ),
        item("demo:3", "Plan: weekly review on Sunday", "A short plan for the Sunday review ritual.", "Chief of Staff", "personal", "plan", &[], None),
    ];
    Needs { count: items.len(), items }
}

pub fn work() -> Work {
    Work { on: true, manual: false, label: Some("On until Fri 0:00".into()) }
}
