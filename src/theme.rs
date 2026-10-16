use gpui::{rgb, rgba, Hsla, WindowAppearance};

pub const SANS: &str = "Geist";
pub const MONO: &str = "Geist Mono";
pub const ICONS: &str = "Segoe MDL2 Assets";

#[derive(Clone, Copy)]
pub struct Theme {
    pub dark: bool,
    pub app: Hsla,
    pub panel: Hsla,
    pub stage: Hsla,
    pub field: Hsla,
    pub text: Hsla,
    pub text2: Hsla,
    pub text3: Hsla,
    pub border: Hsla,
    pub hud_border: Hsla,
    pub accent: Hsla,
    pub on_accent: Hsla,
    pub success: Hsla,
}

impl Theme {
    pub fn from(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::dark(),
            _ => Self::light(),
        }
    }

    pub fn dark() -> Self {
        Self {
            dark: true,
            app: rgb(0x111111).into(),
            panel: rgb(0x191919).into(),
            stage: rgb(0x141414).into(),
            field: rgb(0x222222).into(),
            text: rgb(0xeeeeee).into(),
            text2: rgb(0xb4b4b4).into(),
            text3: rgb(0x6e6e6e).into(),
            border: rgba(0xffffff12).into(),
            hud_border: rgba(0xffffff1a).into(),
            accent: rgb(0x2f6fed).into(),
            on_accent: rgb(0xffffff).into(),
            success: rgb(0x1a7f37).into(),
        }
    }

    pub fn light() -> Self {
        Self {
            dark: false,
            app: rgb(0xf3f3f3).into(),
            panel: rgb(0xffffff).into(),
            stage: rgb(0xfafafa).into(),
            field: rgb(0xf2f2f2).into(),
            text: rgb(0x202020).into(),
            text2: rgb(0x646464).into(),
            text3: rgb(0xbbbbbb).into(),
            border: rgba(0x00000012).into(),
            hud_border: rgba(0x00000014).into(),
            accent: rgb(0x2f6fed).into(),
            on_accent: rgb(0xffffff).into(),
            success: rgb(0x1a7f37).into(),
        }
    }

    /// The logo color: black on light, white on dark.
    pub fn mark(&self) -> Hsla {
        if self.dark {
            rgb(0xffffff).into()
        } else {
            rgb(0x111111).into()
        }
    }
}
