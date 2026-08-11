//! Shared browser WRITE flows — the exact call sequences `gui/src/vm_host.rs`'s
//! handler arms perform against an [`Image`], extracted so the Cocoa GUI's
//! browser drives the SAME implementation instead of a second one (dual
//! placement covers renderers, never the write logic). Each function is thin
//! orchestration over the existing `Image` methods + `mst` parsers; all real
//! work (versioning, SQL, parsing) stays where it already lives.
//!
//! LIVE-compile is deliberately NOT here: each GUI owns its path to its
//! running VM (`vm_host::live_compile` = `vm.exec`; the Cocoa browser ships
//! the same source text as a `#doit` to its primary). The `reopen_*` builders
//! below produce that source text, shared so both compile the same thing.

use crate::mst::{parse_method_selector, parse_mst_source};
use crate::{Image, Side};

/// The `source_file` a browser-created method gets when the caller has no
/// real `.mst`-file identity to offer (`docs/package_aware_editing_design.md`
/// §4.2) — every method gets *some* home, even one invented at the moment of
/// creation, so `Image::class_home_file` is never left with nothing to
/// derive from just because a class didn't come from the importer.
pub const INTERACTIVE_SOURCE_FILE: &str = "interactive.mst";

/// The editor's whole-class save: parse the buffer, write only the DIFF.
///
/// # Why this is here rather than in a GUI
///
/// It was in two GUIs. `gui`'s web editor has this logic and
/// `cocoa_gui/src/host_service.rs` copied it, describing itself as *"the web
/// GUI's `persist_editor_class` logic, over the shared `image_store` API"*.
/// WG6c would have made it three, which is the failure the CG8 gate exists to
/// prevent one level up: not two GUIs writing the image differently, but three
/// implementations of HOW to write it, drifting independently.
///
/// This module's own header already argued for the move — *"extracted so the
/// Cocoa GUI's browser drives the SAME implementation instead of a second
/// one (dual placement covers renderers, never the write logic)"*. The editor
/// save simply never made it here.
///
/// **Promoted FAITHFULLY.** The behaviour is byte-for-byte the Mac's,
/// including the parts worth improving (see below), so that migrating the
/// other two callers onto it is a pure delete-and-delegate with nothing to
/// re-verify. Changing semantics in the same commit that consolidates them
/// would be two changes wearing one hat, and would leave the untouched copies
/// differing in a way nobody would notice.
///
/// **Known weakness, recorded rather than fixed here:** every image call is
/// `let _ = ...`, so a failed write is invisible and the summary still reads
/// as success. That is the existing behaviour in both current callers. It
/// should be given a `Result`, once, when all three call this one.
///
/// # What it does
///
/// Sets the class shell (superclass + instance variables — the one edit a
/// live redefinition cannot do, which is why Save goes to the database and
/// takes effect on the next boot), then writes only what CHANGED: a
/// byte-identical method keeps its version, so there is no history churn and
/// no nmethod invalidation. Methods that vanished from the buffer are
/// removed. Answers a one-line summary for the transcript.
pub fn persist_editor_class(img: &Image, text: &str) -> String {
    use std::collections::{HashMap, HashSet};
    let parsed = crate::mst::parse_mst_source(text);
    let cls = match parsed.first() {
        Some(c) => c,
        None => return "nothing to save (no class definition in the buffer)".to_string(),
    };
    let _ = img.create_or_reopen_class(
        &cls.name,
        cls.superclass.as_deref(),
        "",
        "",
        &cls.instance_vars,
    );
    let _ = img.set_class_definition(&cls.name, cls.superclass.as_deref(), &cls.instance_vars);

    let stored: HashMap<(String, bool), String> = img
        .all_methods_of(&cls.name)
        .unwrap_or_default()
        .into_iter()
        .map(|m| ((m.selector, m.side == Side::Class), m.source))
        .collect();

    let (mut changed, mut added) = (0u32, 0u32);
    for m in &cls.methods {
        let side = if m.is_class_side {
            Side::Class
        } else {
            Side::Instance
        };
        match stored.get(&(m.selector.clone(), m.is_class_side)) {
            // Byte-identical: left completely alone. This is the whole point
            // of diffing rather than rewriting -- an untouched method keeps
            // its version and its history.
            Some(old) if *old == m.source => {}
            Some(_) => {
                let _ = img.create_or_reopen_method(&cls.name, side, &m.selector, "", &m.source);
                changed += 1;
            }
            None => {
                let _ = img.create_or_reopen_method(&cls.name, side, &m.selector, "", &m.source);
                added += 1;
            }
        }
    }

    let present: HashSet<(String, bool)> = cls
        .methods
        .iter()
        .map(|m| (m.selector.clone(), m.is_class_side))
        .collect();
    let mut removed = 0u32;
    for (sel, is_cls) in stored.keys() {
        if !present.contains(&(sel.clone(), *is_cls)) {
            let side = if *is_cls { Side::Class } else { Side::Instance };
            let _ = img.remove_method(&cls.name, side, sel);
            removed += 1;
        }
    }

    if changed + added + removed == 0 {
        format!("{} — no changes", cls.name)
    } else {
        format!(
            "saved {} ({changed} changed, {added} added, {removed} removed)",
            cls.name
        )
    }
}

/// `vm_host`'s `is_valid_var_name`, verbatim: a Smalltalk-style identifier
/// (a letter or `_`, then letters/digits/`_`).
pub fn is_valid_var_name(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `vm_host`'s `reopen_one_method`, superclass supplied by the caller (read
/// from the image, exactly as the web reads it from its mirror): the .mst
/// reopen that live-compiles ONE method into an existing class.
pub fn reopen_one_method(superclass: &str, class_name: &str, method_text: &str) -> String {
    format!(
        "{superclass} subclass: {class_name} [\n{}\n]\n",
        method_text.trim()
    )
}

/// The class-variable reopen `vm_host`'s `SmapplAddVar` arm live-compiles
/// (a class var is an association, not instance shape — reopening appends it
/// cleanly and takes effect immediately).
pub fn class_var_reopen(superclass: &str, class_name: &str, var_name: &str) -> String {
    format!("{superclass} subclass: {class_name} [\n    <classVars: {var_name}>\n]")
}

/// New class from accepted `.mst` text — the `SmapplNewClass` /
/// `BrowserSaveSource::NewClass` image sequence: parse, refuse a duplicate,
/// `create_or_reopen_class`, then each parsed method via
/// `create_or_reopen_method`. Answers the new class's name.
///
/// `default_source_file` is each new method's `source_file`
/// (`docs/package_aware_editing_design.md` §4.2) — pass a real `.mst`-file
/// name if the caller's browser is scoped to one, or `None` to fall back to
/// [`INTERACTIVE_SOURCE_FILE`]. Either way every method gets *some* home, so
/// `Image::class_home_file` is never `None` just because a class was created
/// here rather than imported.
pub fn new_class_from_source(
    img: &Image,
    text: &str,
    default_source_file: Option<&str>,
) -> Result<String, String> {
    let Some(pc) = parse_mst_source(text).into_iter().next() else {
        return Err("Could not parse the class definition — check the syntax.".to_string());
    };
    if matches!(img.class_named(&pc.name), Ok(Some(_))) {
        return Err(format!("A class named {} already exists.", pc.name));
    }
    img.create_or_reopen_class(
        &pc.name,
        pc.superclass.as_deref(),
        "",
        "",
        &pc.instance_vars,
    )
    .map_err(|e| e.to_string())?;
    let source_file = default_source_file.unwrap_or(INTERACTIVE_SOURCE_FILE);
    let mut failed = 0usize;
    for m in &pc.methods {
        let side = if m.is_class_side {
            Side::Class
        } else {
            Side::Instance
        };
        if img
            .create_or_reopen_method(
                &pc.name,
                side,
                &m.selector,
                "as yet unclassified",
                &m.source,
            )
            .is_err()
        {
            failed += 1;
            continue;
        }
        let _ = img.set_method_home_file(&pc.name, side, &m.selector, source_file);
    }
    if failed > 0 {
        return Err(format!(
            "Created {}, but {failed} method(s) failed to persist.",
            pc.name
        ));
    }
    Ok(pc.name)
}

/// New-or-edited method from accepted text — the `BrowserSaveSource::NewMethod`
/// image sequence: parse the message pattern off the text, then
/// `create_or_reopen_method` (idempotent create-or-update, versioned). The
/// text may carry the in-body class-side form (`Cls class >> sel [ … ]`) —
/// the stored form class-side sources already use — so the pattern is parsed
/// from the text with any `Cls class >>` prefix stripped first. Answers the
/// selector.
///
/// `default_source_file`: see [`new_class_from_source`]'s doc comment — same
/// contract, applied to this one method. An EDIT of an already-imported
/// method (one that already has a real `source_file`) keeps it rather than
/// being overwritten with the synthetic marker — only a method with no home
/// yet gets one assigned here.
pub fn save_method(
    img: &Image,
    class_name: &str,
    side: Side,
    text: &str,
    default_source_file: Option<&str>,
) -> Result<String, String> {
    let pattern_text = text
        .trim_start()
        .strip_prefix(&format!("{class_name} class >>"))
        .map(str::trim_start)
        .unwrap_or(text);
    let Some(selector) = parse_method_selector(pattern_text) else {
        return Err("Could not parse a message pattern from the method source.".to_string());
    };
    match img.create_or_reopen_method(class_name, side, &selector, "as yet unclassified", text) {
        Ok(Some(_)) => {
            // Only assign a home if this exact method has none yet — an
            // edit of an already-imported method must never have its real
            // source_file clobbered by the synthetic marker.
            let has_home = img
                .method_source_file(class_name, side, &selector)
                .ok()
                .flatten()
                .is_some();
            if !has_home {
                let source_file = default_source_file.unwrap_or(INTERACTIVE_SOURCE_FILE);
                let _ = img.set_method_home_file(class_name, side, &selector, source_file);
            }
            Ok(selector)
        }
        Ok(None) => Err(format!("No class named {class_name} in the image.")),
        Err(e) => Err(e.to_string()),
    }
}

/// Add a variable — the `SmapplAddVar` image sequence: validate the name,
/// then `reimport_class_shell` (the idempotent union into the stored shell).
/// An INSTANCE variable changes shape and so takes effect on the next boot
/// (no `become:`); a CLASS variable can additionally be made live by
/// compiling [`class_var_reopen`] — the caller's half.
pub fn add_variable(
    img: &Image,
    class_name: &str,
    is_class_var: bool,
    name: &str,
) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Enter a variable name.".to_string());
    }
    if !is_valid_var_name(name) {
        return Err(format!(
            "'{name}' is not a valid variable name (letters, digits, _)."
        ));
    }
    if !matches!(img.class_named(class_name), Ok(Some(_))) {
        return Err(format!("No class named {class_name}."));
    }
    let (iv, cv) = if is_class_var { ("", name) } else { (name, "") };
    match img.reimport_class_shell(class_name, iv, cv) {
        Ok(true) => Ok(()),
        Ok(false) => Err(format!("No class named {class_name}.")),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img() -> Image {
        let img = Image::open_in_memory().expect("in-memory image");
        img.create_or_reopen_class("Object", None, "", "", "")
            .expect("seed Object");
        img
    }

    #[test]
    fn new_class_parses_creates_and_refuses_duplicates() {
        let img = img();
        let name = new_class_from_source(
            &img,
            "Object subclass: Painter [\n    |brush|\n    stroke [ ^1 ]\n]",
            None,
        )
        .expect("create");
        assert_eq!(name, "Painter");
        let cls = img.class_named("Painter").unwrap().expect("stored");
        assert_eq!(cls.instance_vars, "brush");
        assert_eq!(
            img.method_source("Painter", Side::Instance, "stroke")
                .unwrap(),
            Some("stroke [ ^1 ]".to_string())
        );
        assert!(new_class_from_source(&img, "Object subclass: Painter [\n]", None).is_err());
        assert!(new_class_from_source(&img, "not a class at all", None).is_err());
    }

    #[test]
    fn new_class_from_source_gives_every_method_a_home() {
        // M3 (docs/package_aware_editing_design.md §4.2): a class created
        // interactively must never leave class_home_file unanswerable.
        let img = img();
        new_class_from_source(
            &img,
            "Object subclass: Painter [\n    stroke [ ^1 ]\n]",
            None,
        )
        .expect("create");
        assert_eq!(
            img.class_home_file("Painter").unwrap(),
            Some(INTERACTIVE_SOURCE_FILE.to_string())
        );

        // A caller with a real scope (e.g. a package-scoped browser) gets
        // that identity recorded instead of the synthetic default.
        new_class_from_source(
            &img,
            "Object subclass: Widget [\n    draw [ ^1 ]\n]",
            Some("99_scratch.mst"),
        )
        .expect("create");
        assert_eq!(
            img.class_home_file("Widget").unwrap(),
            Some("99_scratch.mst".to_string())
        );
    }

    #[test]
    fn save_method_parses_pattern_including_class_side_prefix() {
        let img = img();
        new_class_from_source(&img, "Object subclass: Painter [\n]", None).expect("create");
        assert_eq!(
            save_method(&img, "Painter", Side::Instance, "dab: x [ ^x ]", None).expect("save"),
            "dab:"
        );
        assert_eq!(
            save_method(
                &img,
                "Painter",
                Side::Class,
                "Painter class >> make [ ^self new ]",
                None
            )
            .expect("save class side"),
            "make"
        );
        assert!(save_method(&img, "Painter", Side::Instance, "", None).is_err());
        assert!(save_method(&img, "NoSuch", Side::Instance, "x [ ^1 ]", None).is_err());
    }

    #[test]
    fn save_method_homes_a_new_method_but_never_reclobbers_an_imported_ones_home() {
        let img = img();
        new_class_from_source(&img, "Object subclass: Painter [\n]", None).expect("create");
        // A brand-new method via save_method: gets the synthetic home.
        save_method(&img, "Painter", Side::Instance, "dab: x [ ^x ]", None).expect("save");
        assert_eq!(
            img.method_source_file("Painter", Side::Instance, "dab:")
                .unwrap(),
            Some(INTERACTIVE_SOURCE_FILE.to_string())
        );

        // A method that already has REAL provenance (as if imported) keeps
        // it across an edit through save_method, even with no explicit
        // default_source_file passed — the edit must never look like it
        // originated interactively when it didn't.
        img.set_method_home_file("Painter", Side::Instance, "dab:", "12_real.mst")
            .unwrap();
        save_method(&img, "Painter", Side::Instance, "dab: x [ ^x + 1 ]", None).expect("re-save");
        assert_eq!(
            img.method_source_file("Painter", Side::Instance, "dab:")
                .unwrap(),
            Some("12_real.mst".to_string()),
            "editing an already-homed method must not clobber its real source_file"
        );
    }

    #[test]
    fn add_variable_validates_and_unions() {
        let img = img();
        new_class_from_source(&img, "Object subclass: Painter [\n    |brush|\n]", None)
            .expect("create");
        add_variable(&img, "Painter", false, "canvas").expect("add ivar");
        add_variable(&img, "Painter", true, "Palette").expect("add classvar");
        let cls = img.class_named("Painter").unwrap().expect("stored");
        assert!(cls.instance_vars.split_whitespace().any(|v| v == "canvas"));
        assert!(cls.instance_vars.split_whitespace().any(|v| v == "brush"));
        assert!(cls.class_vars.split_whitespace().any(|v| v == "Palette"));
        assert!(add_variable(&img, "Painter", false, "9bad").is_err());
        assert!(add_variable(&img, "Painter", false, "").is_err());
        assert!(add_variable(&img, "NoSuch", false, "x").is_err());
    }

    #[test]
    fn reopen_builders_match_vm_host_shapes() {
        assert_eq!(
            reopen_one_method("Object", "Painter", "dab: x [ ^x ]\n"),
            "Object subclass: Painter [\ndab: x [ ^x ]\n]\n"
        );
        assert_eq!(
            class_var_reopen("Object", "Painter", "Palette"),
            "Object subclass: Painter [\n    <classVars: Palette>\n]"
        );
    }
}
