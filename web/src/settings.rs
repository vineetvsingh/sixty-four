//! Look and feel: board colours, piece set, how fast pieces slide, and volume.
//! Kept in localStorage and applied to every board on the site.

use super::*;

const SETTINGS_KEY: &str = "sixtyfour-settings";
/// Board colour schemes; the colours themselves are in index.html under `data-board`.
const BOARDS: [&str; 4] = ["blue", "wood", "green", "slate"];
/// Piece sets: the first lives in static/pieces, the others in a folder of their own.
const PIECE_SETS: [&str; 3] = ["cburnett", "merida", "chessnut"];
/// Milliseconds a piece takes to slide: slow, normal, fast, and not animated at all.
const SPEEDS: [f64; 4] = [600.0, 380.0, 200.0, 0.0];

pub struct Settings {
    pub board: usize,
    pub pieces: usize,
    pub speed: usize,
    /// 0.0 to 1.0.
    pub volume: f64,
}

impl Settings {
    pub fn load() -> Settings {
        let mut s = Settings { board: 0, pieces: 0, speed: 1, volume: 1.0 };
        if let Some(v) = local_get(SETTINGS_KEY).and_then(|raw| js_sys::JSON::parse(&raw).ok()) {
            let num = |k: &str| js_sys::Reflect::get(&v, &JsValue::from_str(k)).ok().and_then(|x| x.as_f64());
            let index = |k: &str, len: usize, default: usize| num(k).map(|n| n as usize).filter(|n| *n < len).unwrap_or(default);
            s.board = index("board", BOARDS.len(), 0);
            s.pieces = index("pieces", PIECE_SETS.len(), 0);
            s.speed = index("speed", SPEEDS.len(), 1);
            s.volume = num("volume").unwrap_or(1.0).clamp(0.0, 1.0);
        }
        s
    }

    fn save(&self) {
        let json = format!(
            "{{\"board\":{},\"pieces\":{},\"speed\":{},\"volume\":{:.2}}}",
            self.board, self.pieces, self.speed, self.volume
        );
        if let Ok(Some(store)) = window().local_storage() {
            let _ = store.set_item(SETTINGS_KEY, &json);
        }
    }

    /// Make the page look and move the way the settings say.
    pub fn apply(&self) {
        if let Some(root) = doc().document_element() {
            let _ = root.set_attribute("data-board", BOARDS[self.board]);
        }
        // The default set is styled in index.html; another set overrides it from here.
        let css: String = match self.pieces {
            0 => String::new(),
            set => ["wK", "wQ", "wR", "wB", "wN", "wP", "bK", "bQ", "bR", "bB", "bN", "bP"]
                .iter()
                .map(|p| format!(".{p}{{background-image:url(pieces/{}/{p}.svg)}}", PIECE_SETS[set]))
                .collect(),
        };
        set_text("piece-css", &css);
        SPEED.with(|s| s.set(SPEEDS[self.speed]));
    }

    /// Show the current settings in the settings panel.
    pub fn show(&self) {
        for (id, value) in [("set-board", self.board), ("set-pieces", self.pieces), ("set-speed", self.speed)] {
            el(id).unchecked_into::<HtmlSelectElement>().set_value(&value.to_string());
        }
        el("set-volume").unchecked_into::<HtmlInputElement>().set_value(&format!("{:.0}", self.volume * 100.0));
    }
}

impl App {
    /// A control in the settings panel was changed: read them all, apply and remember.
    pub fn settings_changed(&mut self) {
        let pick = |id: &str, len: usize| {
            let v: usize = el(id).unchecked_into::<HtmlSelectElement>().value().parse().unwrap_or(0);
            v.min(len - 1)
        };
        let s = &mut self.settings;
        s.board = pick("set-board", BOARDS.len());
        s.pieces = pick("set-pieces", PIECE_SETS.len());
        s.speed = pick("set-speed", SPEEDS.len());
        let volume: f64 = el("set-volume").unchecked_into::<HtmlInputElement>().value().parse().unwrap_or(100.0);
        s.volume = (volume / 100.0).clamp(0.0, 1.0);
        s.apply();
        s.save();
        self.sound.set_volume(self.settings.volume);
    }

    pub fn settings_open(&mut self, open: bool) {
        set_hidden("settings", !open);
        let _ = el("btn-settings").set_attribute("aria-expanded", if open { "true" } else { "false" });
    }

    pub fn settings_toggle(&mut self) {
        let open = el("settings").has_attribute("hidden");
        self.settings_open(open);
    }
}
