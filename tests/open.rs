//! Opening what a message names, tested without starting a program.

use plumber::message::Message;
use plumber::open::{candidates, on_path, open, split_words, Vars};

fn with_addr(text: &str, addr: &str) -> Message {
    Message {
        attr: vec![("addr".into(), addr.into())],
        ..Message::new("test", "text", text)
    }
}

#[test]
fn a_line_number_from_the_message_fills_the_editor_command() {
    let message = with_addr("C:/work/main.rs", ":42");
    let command = open(
        &message,
        "edit",
        Some("myeditor --goto {file}:{line}"),
        true,
    )
    .unwrap();
    assert_eq!(command, "would run: myeditor --goto C:/work/main.rs:42");
}

#[test]
fn a_message_without_a_line_number_does_not_get_a_dangling_colon() {
    let message = Message::new("test", "text", "C:/work/main.rs");
    let command = open(
        &message,
        "edit",
        Some("myeditor --goto {file}:{line}"),
        true,
    )
    .unwrap();
    assert_eq!(
        command, "would run: myeditor --goto C:/work/main.rs",
        "a missing line number drops the colon, not the file"
    );
}

#[test]
fn a_page_message_opens_the_document_at_that_page() {
    let message = Message {
        attr: vec![("page".into(), ":7".into())],
        ..Message::new("test", "text", "manual.pdf")
    };
    let vars = Vars::from(&message);
    assert_eq!(
        vars.page, "7",
        "the page comes from `page=` without its colon"
    );
    // The template is explicit so the test does not depend on which pdf viewer is installed.
    let command = open(&message, "pdf", Some("pdfcraft {file} --page {page}"), true).unwrap();
    assert_eq!(command, "would run: pdfcraft manual.pdf --page 7");
    // And the built-in chain stays page-aware: whatever viewer a machine has, the first choice
    // knows how to open at a page.
    let chain = candidates("pdf");
    assert!(
        chain
            .iter()
            .any(|candidate| candidate.starts_with("pdfcraft")),
        "pdfcraft must stay in the chain: {chain:?}"
    );
    assert!(
        chain[0].contains("{page}"),
        "the first pdf candidate has to use the page: {chain:?}"
    );
}

#[test]
fn an_empty_argument_is_shown_as_an_empty_argument() {
    let words = split_words("cmd /c start \"\" https://9fans.net");
    assert_eq!(
        plumber::open::display(&words),
        "cmd /c start \"\" https://9fans.net"
    );
}

#[test]
fn an_attribute_can_be_named_in_a_template() {
    let message = Message {
        attr: vec![("part".into(), "XB-24".into())],
        ..Message::new("test", "text", "XB-24")
    };
    let command = open(
        &message,
        "catalog",
        Some("catalog --part {attr:part}"),
        true,
    )
    .unwrap();
    assert_eq!(command, "would run: catalog --part XB-24");
}

#[test]
fn a_quoted_empty_argument_survives_the_split() {
    // Windows `start` needs an empty title before a URL, or it treats the URL as the title.
    let words = split_words("cmd /c start \"\" https://9fans.net");
    assert_eq!(words, vec!["cmd", "/c", "start", "", "https://9fans.net"]);
}

#[test]
fn a_default_opener_is_found_even_when_the_first_program_is_missing() {
    // `code` is not installed on most machines; the list must fall through to something that is.
    let message = with_addr("C:/work/main.rs", ":42");
    let command = open(&message, "edit", None, true).unwrap();
    assert!(command.starts_with("would run: "), "{command}");
    let program = command
        .trim_start_matches("would run: ")
        .split(' ')
        .next()
        .unwrap();
    assert!(on_path(program), "{program} is claimed to be on PATH");
}

#[test]
fn a_program_that_does_not_exist_is_not_on_path() {
    assert!(!on_path("definitely-not-a-program-xyzzy"));
}

#[test]
fn a_port_with_no_opener_says_so() {
    let message = Message::new("test", "text", "whatever");
    let error = open(&message, "nothing", None, true).unwrap_err();
    assert!(error.contains("no default opener"), "{error}");
    assert!(
        candidates("edit").len() >= 3,
        "the edit port keeps its fallback chain"
    );
}

#[test]
fn every_port_offers_a_chain_that_reaches_a_desktop() {
    // The built-in chain has to work on Linux, macOS and Windows without a per-platform build:
    // whichever program is installed wins, and at least one fallback is always portable.
    assert!(candidates("edit").contains(&"vim +{line} {file}"));
    assert!(candidates("web").contains(&"xdg-open {data}"));
    assert!(candidates("web").contains(&"open {data}"));
    assert!(candidates("pdf").contains(&"zathura -P {page} {file}"));
    assert!(candidates("unknown-port").is_empty());
}

#[test]
fn a_missing_line_number_drops_vims_plus() {
    // `vim + file` means "jump to the last line"; dropping the word keeps the file and the
    // behaviour sane, exactly like the dangling colon of `code -g`.
    let message = Message::new("test", "text", "/tmp/main.rs");
    let command = open(&message, "edit", Some("vim +{line} {file}"), true).unwrap();
    assert_eq!(command, "would run: vim /tmp/main.rs");
}

#[test]
fn a_line_number_keeps_vims_plus() {
    let message = with_addr("/tmp/main.rs", ":42");
    let command = open(&message, "edit", Some("vim +{line} {file}"), true).unwrap();
    assert_eq!(command, "would run: vim +42 /tmp/main.rs");
}
