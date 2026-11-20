//! Translations with gettext. Mark strings with `tr("…")`; `trf("… {} …", &[..])` fills `{}` in order.
//! `po/update.sh` extracts both keywords.

use crate::paths;
use gettextrs::{LocaleCategory, bind_textdomain_codeset, bindtextdomain, gettext, ngettext, setlocale, textdomain};

pub fn init() {
    // SAFETY: called once at start-up, before any other thread exists.
    unsafe {
        setlocale(LocaleCategory::LcAll, "");
    }
    let _ = bindtextdomain(paths::GETTEXT_PACKAGE, paths::locale_dir());
    let _ = bind_textdomain_codeset(paths::GETTEXT_PACKAGE, "UTF-8");
    let _ = textdomain(paths::GETTEXT_PACKAGE);
}

pub fn tr(msgid: &str) -> String {
    gettext(msgid)
}

pub fn trf(msgid: &str, args: &[&str]) -> String {
    let mut out = gettext(msgid);
    for arg in args {
        out = out.replacen("{}", arg, 1);
    }
    out
}

/// Plural-aware `trf`: picks `singular` or `plural` for `n` in the user's language.
pub fn ntrf(singular: &str, plural: &str, n: u32, args: &[&str]) -> String {
    let mut out = ngettext(singular, plural, n);
    for arg in args {
        out = out.replacen("{}", arg, 1);
    }
    out
}
