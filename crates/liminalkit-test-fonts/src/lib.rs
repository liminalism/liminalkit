//! Two Noto faces for liminalkit's tests and examples.
//!
//! This crate is never published (`publish = false`) and is only ever a
//! dev-dependency, so the 550 KB of font data stays out of every crate that
//! goes to crates.io and out of every application build. An application
//! embeds its own faces with [`Face::from_bytes`] and registers them in a
//! [`FontSet`]; this crate is what that looks like with two particular files.
//!
//! | File | Family | Version | Glyphs | Size | Copyright |
//! |---|---|---|---|---|---|
//! | `fonts/NotoSans-Regular.ttf` | Noto Sans Regular | 2.004 | 3317 | 512,672 bytes | 2015 Google LLC |
//! | `fonts/NotoSansThai-Regular.ttf` | Noto Sans Thai Regular | 2.000 | 140 | 37,744 bytes | 2016 Google Inc. |
//!
//! Both are unmodified upstream builds (hinted with ttfautohint 1.8.3, as the
//! files' own version strings say) from the `fonts-noto-core` distribution,
//! licensed under the SIL Open Font License, Version 1.1; `fonts/README.md`
//! has the details. Two faces, not one, because together they exercise what
//! a single face cannot: Noto Sans Thai has no Latin letters or digits and
//! Noto Sans has no Thai, so a price in a Thai string needs fallback, and a
//! Thai string needs the `ccmp`, `mark` and `mkmk` lookups the shaper
//! implements.

use liminalkit_text::{Face, FaceId, FontSet, TextStyle};
use std::sync::OnceLock;

/// Noto Sans Thai Regular 2.000.
pub fn thai() -> &'static Face {
    static FACE: OnceLock<Face> = OnceLock::new();
    FACE.get_or_init(|| {
        Face::from_bytes(include_bytes!("../fonts/NotoSansThai-Regular.ttf"))
            .expect("the bundled Noto Sans Thai parses")
    })
}

/// Noto Sans Regular 2.004.
pub fn latin() -> &'static Face {
    static FACE: OnceLock<Face> = OnceLock::new();
    FACE.get_or_init(|| {
        Face::from_bytes(include_bytes!("../fonts/NotoSans-Regular.ttf"))
            .expect("the bundled Noto Sans parses")
    })
}

/// The id of [`latin`] in [`set`].
pub const LATIN: FaceId = FaceId(0);
/// The id of [`thai`] in [`set`].
pub const THAI: FaceId = FaceId(1);

/// Latin first with Thai as fallback, and Thai with Latin as fallback.
pub fn set() -> &'static FontSet {
    static SET: OnceLock<FontSet> = OnceLock::new();
    SET.get_or_init(|| {
        let mut set = FontSet::new();
        let latin = set.add(latin());
        let thai = set.add(thai());
        set.set_fallbacks(latin, &[thai]);
        set.set_fallbacks(thai, &[latin]);
        set
    })
}

/// Latin-styled text at `size` pixels.
pub const fn style(size: f32) -> TextStyle {
    TextStyle::new(LATIN, size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_set_has_the_two_faces_in_the_documented_order() {
        let set = set();
        assert_eq!(set.len(), 2);
        assert!(std::ptr::eq(set.face(LATIN), latin()));
        assert!(std::ptr::eq(set.face(THAI), thai()));
    }

    #[test]
    fn each_face_covers_what_the_other_lacks() {
        assert!(latin().covers('a') && !latin().covers('ก'));
        assert!(thai().covers('ก') && !thai().covers('a'));
        let (face, id) = set().face_for(LATIN, 'ก');
        assert!(std::ptr::eq(face, thai()));
        assert_eq!(id, THAI);
    }
}
