//! Sixty-Four Chess front end. The whole UI lives in one `App` value; every DOM
//! event, timer and worker reply borrows it, updates state and re-renders.

mod clock;
mod content;
mod keys;
mod online;
mod settings;
mod sound;

use clock::{Clock, TIME_CONTROLS};
use content::{LESSONS, OPENINGS, PUZZLES};
use online::Online;
use settings::Settings;
use sound::{At, Sfx, Sound};
use engine::{name, sq_of, strip, validate_fen, Engine, Move, SearchOpts, SearchOut, Status, START};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{
    Document, Element, Event, EventTarget, FocusEvent, HtmlElement, HtmlInputElement, HtmlSelectElement, HtmlTextAreaElement, KeyboardEvent,
    MessageEvent, PointerEvent, Window, Worker, WorkerOptions, WorkerType,
};

/// Once the player's piece has landed the computer waits at least this long before
/// answering, so its move does not land on top of theirs.
const REPLY_PAUSE_MS: f64 = 570.0;
const SOUND_KEY: &str = "sixtyfour-sound";
const GAME_KEY: &str = "sixtyfour-game";
const FILES: &str = "abcdefgh";
const STORE_KEY: &str = "sixtyfour";
const COACH_INTRO: &str = "Make a move. The coach rates each one and shows what the engine preferred.";

const PLAY: usize = 0;
const LEARN: usize = 1;
const OPEN: usize = 2;
const PUZZLE: usize = 3;
const VIEWS: [&str; 4] = ["play", "learn", "openings", "puzzles"];

const ANALYSIS: SearchOpts = SearchOpts { max_depth: 4, noise: 0, time_ms: 350.0 };
const LEVELS: [SearchOpts; 5] = [
    SearchOpts { max_depth: 1, noise: 220, time_ms: 8000.0 },
    SearchOpts { max_depth: 2, noise: 90, time_ms: 8000.0 },
    SearchOpts { max_depth: 3, noise: 25, time_ms: 8000.0 },
    SearchOpts { max_depth: 5, noise: 0, time_ms: 800.0 },
    SearchOpts { max_depth: 8, noise: 0, time_ms: 2200.0 },
];

fn now() -> f64 {
    js_sys::Date::now()
}
/// How long a piece takes to slide to its square, in milliseconds. It is a setting.
fn move_ms() -> f64 {
    SPEED.with(|s| s.get())
}
fn min_reply_ms() -> f64 {
    move_ms() + REPLY_PAUSE_MS
}
fn rnd() -> f64 {
    js_sys::Math::random()
}
fn new_engine() -> Engine {
    Engine::new(now, rnd)
}

// ---------------------------------------------------------------- search worker

thread_local! {
    static SEARCHER: RefCell<Engine> = RefCell::new(new_engine());
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    static SPEED: Cell<f64> = const { Cell::new(380.0) };
}

fn run_search(fen: &str, opts: SearchOpts) -> Option<SearchOut> {
    SEARCHER.with(|e| {
        let mut e = e.borrow_mut();
        e.load(fen);
        e.search(opts)
    })
}

fn encode_request(id: u32, fen: &str, o: SearchOpts) -> String {
    format!("{id}|{}|{}|{}|{fen}", o.max_depth, o.noise, o.time_ms)
}

fn decode_reply(s: &str) -> Option<(u32, Option<SearchOut>)> {
    let p: Vec<&str> = s.split('|').collect();
    let id = p.first()?.parse().ok()?;
    if p.len() < 7 {
        return Some((id, None));
    }
    Some((
        id,
        Some(SearchOut {
            from: p[1].parse().ok()?,
            to: p[2].parse().ok()?,
            promo: p[3].parse().ok()?,
            score: p[4].parse().ok()?,
            depth: p[5].parse().ok()?,
            san: p[6].to_string(),
        }),
    ))
}

/// Entry point used by worker.js: takes an encoded request, returns an encoded reply.
#[wasm_bindgen]
pub fn worker_search(req: &str) -> String {
    let p: Vec<&str> = req.splitn(5, '|').collect();
    if p.len() < 5 {
        return String::from("0|none");
    }
    let opts = SearchOpts {
        max_depth: p[1].parse().unwrap_or(3),
        noise: p[2].parse().unwrap_or(0),
        time_ms: p[3].parse().unwrap_or(1000.0),
    };
    match run_search(p[4], opts) {
        Some(r) => format!("{}|{}|{}|{}|{}|{}|{}", p[0], r.from, r.to, r.promo, r.score, r.depth, r.san),
        None => format!("{}|none", p[0]),
    }
}

// ---------------------------------------------------------------- DOM helpers

fn window() -> Window {
    web_sys::window().expect("no window")
}
fn doc() -> Document {
    window().document().expect("no document")
}
fn el(id: &str) -> Element {
    doc().get_element_by_id(id).unwrap_or_else(|| panic!("missing #{id}"))
}
fn set_html(id: &str, html: &str) {
    el(id).set_inner_html(html);
}
fn set_text(id: &str, text: &str) {
    el(id).set_text_content(Some(text));
}
fn set_disabled(id: &str, off: bool) {
    let e = el(id);
    if off {
        let _ = e.set_attribute("disabled", "");
    } else {
        let _ = e.remove_attribute("disabled");
    }
}
fn set_hidden(id: &str, hidden: bool) {
    let e = el(id);
    if hidden {
        let _ = e.set_attribute("hidden", "");
    } else {
        let _ = e.remove_attribute("hidden");
    }
}
fn json_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}
/// Ask the browser to put `text` on the clipboard. `None` where it offers no clipboard,
/// which is the case on pages not served over HTTPS or from localhost.
fn clipboard_write(text: &str) -> Option<js_sys::Promise> {
    let clip = js_sys::Reflect::get(&window().navigator(), &JsValue::from_str("clipboard")).unwrap_or(JsValue::UNDEFINED);
    js_sys::Reflect::get(&clip, &JsValue::from_str("writeText"))
        .ok()
        .and_then(|f| f.dyn_into::<js_sys::Function>().ok())
        .and_then(|f| f.call1(&clip, &JsValue::from_str(text)).ok())
        .and_then(|p| p.dyn_into::<js_sys::Promise>().ok())
}
/// Put `text` in the save-and-load box, selected, and on the clipboard where the browser allows it.
fn offer_text(text: &str, what: &'static str) {
    let area = el("io-text").unchecked_into::<HtmlTextAreaElement>();
    area.set_value(text);
    area.select();
    let fallback = format!("{what} is in the box below. Press Ctrl+C to copy it.");
    match clipboard_write(text) {
        Some(promise) => spawn_local(async move {
            let msg = match JsFuture::from(promise).await {
                Ok(_) => format!("{what} copied to the clipboard."),
                Err(_) => fallback,
            };
            set_text("io-msg", &msg);
        }),
        None => set_text("io-msg", &fallback),
    }
}
fn set_style(e: &Element, prop: &str, value: &str) {
    if let Some(h) = e.dyn_ref::<HtmlElement>() {
        let _ = h.style().set_property(prop, value);
    }
}
fn verdict(id: &str, cls: &str, label: &str, text: &str) {
    set_html(id, &format!("<span class=\"chip {cls}\">{label}</span><span>{text}</span>"));
}
fn san_tag(s: &str) -> String {
    format!("<span class=\"san\">{s}</span>")
}
/// CSS class that paints a piece, e.g. `wK` or `bN` (see static/pieces).
fn piece_class(p: u8) -> String {
    let t = (p & 7) as usize;
    format!("{}{}", if p >> 3 == 1 { 'b' } else { 'w' }, &"?PNBRQK"[t..t + 1])
}
/// Squares that change hands in a move: the piece itself, plus the rook when castling.
fn move_pairs(m: Move) -> Vec<(usize, usize)> {
    let (from, to) = (m.from as usize, m.to as usize);
    match m.flag {
        3 => vec![(from, to), (to + 1, to - 1)],
        4 => vec![(from, to), (to - 2, to + 1)],
        _ => vec![(from, to)],
    }
}
fn back_pairs(m: Move) -> Vec<(usize, usize)> {
    move_pairs(m).into_iter().map(|(a, b)| (b, a)).collect()
}
/// Sound for a move that has just been played on `g`: what kind of move it was, and
/// how hard the piece comes down (0 quietly, 1 with check, 2 with checkmate).
fn sfx_for(g: &mut Engine, m: Move) -> (Sfx, u8) {
    let kind = if m.cap != 0 {
        Sfx::Capture
    } else if m.flag >= 3 {
        Sfx::Castle
    } else if m.promo != 0 {
        Sfx::Promote
    } else {
        Sfx::Move
    };
    let force = match (g.in_check(), g.legal().is_empty()) {
        (false, _) => 0,
        (true, false) => 1,
        (true, true) => 2,
    };
    (kind, force)
}
fn local_get(key: &str) -> Option<String> {
    window().local_storage().ok().flatten().and_then(|s| s.get_item(key).ok().flatten())
}
/// Index stored in a `data-*` attribute of the clicked element or an ancestor matching `sel`.
fn data_index(e: &Event, sel: &str, attr: &str) -> Option<usize> {
    let t = e.target()?.dyn_into::<Element>().ok()?;
    t.closest(sel).ok()??.get_attribute(attr)?.parse().ok()
}

fn with_app(f: impl FnOnce(&mut App)) {
    APP.with(|a| {
        if let Ok(mut guard) = a.try_borrow_mut() {
            if let Some(app) = guard.as_mut() {
                f(app);
            }
        }
    });
}

fn listen<E: JsCast + 'static>(target: &EventTarget, event: &str, mut f: impl FnMut(&mut App, E) + 'static) {
    let c = Closure::<dyn FnMut(Event)>::new(move |e: Event| {
        let e = e.unchecked_into::<E>();
        with_app(|a| f(a, e));
    });
    let _ = target.add_event_listener_with_callback(event, c.as_ref().unchecked_ref());
    c.forget();
}

fn on_click(id: &str, mut f: impl FnMut(&mut App) + 'static) {
    listen::<Event>(&el(id), "click", move |a, _| f(a));
}

fn set_timeout(ms: i32, f: impl FnOnce() + 'static) {
    let c = Closure::once_into_js(f);
    let _ = window().set_timeout_with_callback_and_timeout_and_arguments_0(c.unchecked_ref(), ms);
}

fn set_interval(ms: i32, f: impl FnMut() + 'static) {
    let c = Closure::<dyn FnMut()>::new(f);
    let _ = window().set_interval_with_callback_and_timeout_and_arguments_0(c.as_ref().unchecked_ref(), ms);
    c.forget();
}

// ---------------------------------------------------------------- progress (localStorage)

#[derive(Default)]
struct Progress {
    lessons: HashSet<String>,
    puzzles: HashSet<String>,
}

impl Progress {
    fn load() -> Progress {
        let mut p = Progress::default();
        let raw = window().local_storage().ok().flatten().and_then(|s| s.get_item(STORE_KEY).ok().flatten());
        if let Some(v) = raw.and_then(|r| js_sys::JSON::parse(&r).ok()) {
            for (key, set) in [("lessons", &mut p.lessons), ("puzzles", &mut p.puzzles)] {
                if let Ok(o) = js_sys::Reflect::get(&v, &JsValue::from_str(key)) {
                    if let Some(o) = o.dyn_ref::<js_sys::Object>() {
                        set.extend(js_sys::Object::keys(o).iter().filter_map(|k| k.as_string()));
                    }
                }
            }
        }
        p
    }

    fn save(&self) {
        let obj = |s: &HashSet<String>| s.iter().map(|k| format!("\"{k}\":1")).collect::<Vec<_>>().join(",");
        let json = format!("{{\"lessons\":{{{}}},\"puzzles\":{{{}}}}}", obj(&self.lessons), obj(&self.puzzles));
        if let Ok(Some(s)) = window().local_storage() {
            let _ = s.set_item(STORE_KEY, &json);
        }
    }
}

// ---------------------------------------------------------------- board component

struct Drag {
    x: f64,
    y: f64,
    ghost: Option<HtmlElement>,
}

struct BoardView {
    root: Element,
    pos: [u8; 64],
    flipped: bool,
    last: Option<(usize, usize)>,
    check: Option<usize>,
    /// (square, css class) highlights: hint, ring, star, pre.
    marks: Vec<(usize, &'static str)>,
    sel: Option<usize>,
    targets: Vec<Move>,
    drag: Option<Drag>,
    /// Candidate promotions waiting for the player to pick a piece.
    promo: Option<Vec<Move>>,
    /// (from, to) slides in progress, and when they began.
    anim: Vec<(usize, usize)>,
    anim_start: f64,
    /// The pending move was dragged and dropped, so its piece is already in place.
    dropped: bool,
    /// Arrows drawn with the right mouse button; one that starts and ends on the
    /// same square is a circle.
    arrows: Vec<(usize, usize)>,
    /// Square where the right button went down.
    rdown: Option<usize>,
    /// The square the keyboard cursor is on.
    cursor: usize,
    /// The board is being worked with the keyboard: show the cursor and hold the focus.
    kbd: bool,
}

impl BoardView {
    fn new(id: &str) -> BoardView {
        let root = el(id);
        let _ = root.class_list().add_1("board");
        let _ = root.set_attribute("tabindex", "0");
        let _ = root.set_attribute("role", "group");
        let help = "Chess board. Arrow keys move between squares; Enter picks a piece up and puts it down; Escape cancels.";
        let _ = root.set_attribute("aria-label", help);
        BoardView {
            root,
            pos: [0; 64],
            flipped: false,
            last: None,
            check: None,
            marks: Vec::new(),
            sel: None,
            targets: Vec::new(),
            drag: None,
            promo: None,
            anim: Vec::new(),
            anim_start: 0.0,
            dropped: false,
            arrows: Vec::new(),
            rdown: None,
            cursor: 52,
            kbd: false,
        }
    }

    fn clear(&mut self) {
        self.deselect();
        self.anim.clear();
        self.dropped = false;
        self.arrows.clear();
    }

    /// Drop the selection, and with it any promotion being chosen.
    fn deselect(&mut self) {
        self.sel = None;
        self.targets.clear();
        self.promo = None;
    }

    /// Animate these (from, to) pairs on the next renders. Returns the delay until the
    /// first piece lands: zero if the player dropped it there by hand.
    fn slide(&mut self, mut pairs: Vec<(usize, usize)>) -> f64 {
        let dropped = std::mem::take(&mut self.dropped);
        if dropped && !pairs.is_empty() {
            pairs.remove(0);
        }
        self.anim = pairs;
        self.anim_start = now();
        if dropped {
            0.0
        } else {
            move_ms()
        }
    }

    /// Copy the position and check marker from a game.
    fn sync(&mut self, g: &Engine) {
        self.pos = *g.board();
        self.check = if g.in_check() { Some(g.king_sq(g.turn())) } else { None };
    }

    /// The arrows and circles, as a drawing laid over the squares.
    fn arrows_svg(&self) -> String {
        if self.arrows.is_empty() {
            return String::new();
        }
        // Centre of a square in board units (one square is 1 by 1), as it appears on screen.
        let centre = |sq: usize| {
            let i = if self.flipped { 63 - sq } else { sq };
            ((i & 7) as f64 + 0.5, (i >> 3) as f64 + 0.5)
        };
        let mut s = String::from("<svg class=\"marks\" viewBox=\"0 0 8 8\" aria-hidden=\"true\">");
        for &(from, to) in &self.arrows {
            let (x1, y1) = centre(from);
            if from == to {
                s.push_str(&format!("<circle cx=\"{x1}\" cy=\"{y1}\" r=\"0.43\"/>"));
                continue;
            }
            let (x2, y2) = centre(to);
            let len = (x2 - x1).hypot(y2 - y1);
            let (ux, uy) = ((x2 - x1) / len, (y2 - y1) / len);
            // The shaft starts just off the piece and stops where the head begins.
            let (sx, sy) = (x1 + ux * 0.3, y1 + uy * 0.3);
            let (hx, hy) = (x2 - ux * 0.42, y2 - uy * 0.42);
            s.push_str(&format!(
                "<line x1=\"{sx:.3}\" y1=\"{sy:.3}\" x2=\"{hx:.3}\" y2=\"{hy:.3}\"/><polygon points=\"{x2:.3},{y2:.3} {:.3},{:.3} {:.3},{:.3}\"/>",
                hx - uy * 0.22,
                hy + ux * 0.22,
                hx + uy * 0.22,
                hy - ux * 0.22
            ));
        }
        s.push_str("</svg>");
        s
    }

    fn render(&self) {
        let mut h = String::with_capacity(6000);
        // Re-renders during a slide resume it part-way instead of restarting or cutting it short.
        let ms = move_ms();
        let elapsed = now() - self.anim_start;
        let sliding = elapsed < ms;
        for i in 0..64usize {
            let sq = if self.flipped { 63 - i } else { i };
            let (r, f) = (sq >> 3, sq & 7);
            let p = self.pos[sq];
            let mut c = String::from(if (r + f) & 1 == 1 { "sq dk" } else { "sq lt" });
            if self.sel == Some(sq) {
                c.push_str(" sel");
            }
            if matches!(self.last, Some((a, b)) if a == sq || b == sq) {
                c.push_str(" last");
            }
            if self.check == Some(sq) {
                c.push_str(" chk");
            }
            if let Some((_, m)) = self.marks.iter().find(|(s, _)| *s == sq) {
                c.push(' ');
                c.push_str(m);
            }
            if let Some(t) = self.targets.iter().find(|m| m.to as usize == sq) {
                c.push_str(if t.cap != 0 { " tgt cap" } else { " tgt" });
            }
            if self.kbd && self.cursor == sq {
                c.push_str(" kc");
            }
            h.push_str(&format!("<div class=\"{c}\" data-sq=\"{sq}\">"));
            if i % 8 == 0 {
                h.push_str(&format!("<i class=\"rk\">{}</i>", 8 - r));
            }
            if i >= 56 {
                h.push_str(&format!("<i class=\"fl\">{}</i>", &FILES[f..f + 1]));
            }
            if p != 0 {
                let from = self.anim.iter().find(|(_, to)| sliding && *to == sq).map(|(from, _)| *from);
                let slide = match from {
                    Some(from) => {
                        let k = if self.flipped { -100 } else { 100 };
                        let dx = ((from & 7) as i32 - f as i32) * k;
                        let dy = ((from >> 3) as i32 - r as i32) * k;
                        format!(" slide\" style=\"--dx:{dx}%;--dy:{dy}%;animation-duration:{ms}ms;animation-delay:-{elapsed:.0}ms")
                    }
                    None => String::new(),
                };
                h.push_str(&format!("<span class=\"pc {}{slide}\"></span>", piece_class(p)));
            }
            h.push_str("</div>");
        }
        if let Some(pm) = &self.promo {
            let colour = pm[0].piece & 8;
            h.push_str("<div class=\"promo\">");
            for (t, piece) in [(5u8, "queen"), (4, "rook"), (3, "bishop"), (2, "knight")] {
                h.push_str(&format!(
                    "<button data-promo=\"{t}\" aria-label=\"Promote to a {piece}\"><span class=\"pc {}\"></span></button>",
                    piece_class(t | colour)
                ));
            }
            h.push_str("</div>");
        }
        h.push_str(&self.arrows_svg());
        self.root.set_inner_html(&h);
        if self.kbd {
            // Drawing the board again replaces whatever had the focus: put it back
            // where the keyboard user was, or on the promotion choice if there is one.
            let target = match &self.promo {
                Some(_) => self.root.query_selector(".promo button").ok().flatten(),
                None => Some(self.root.clone()),
            };
            if let Some(t) = target.and_then(|t| t.dyn_into::<HtmlElement>().ok()) {
                let has_focus = doc().active_element().is_some_and(|a| a == *t.as_ref());
                let choosing = self.promo.is_some() && doc().active_element().is_some_and(|a| a.closest(".promo").ok().flatten().is_some());
                if !has_focus && !choosing {
                    let _ = t.focus();
                }
            }
        }
    }

    /// Once the pointer has travelled a little, lift a copy of the piece and follow the pointer.
    fn drag_move(&mut self, e: &PointerEvent) {
        let (x, y) = (e.client_x() as f64, e.client_y() as f64);
        let Some(sel) = self.sel else { return };
        let Some(d) = self.drag.as_mut() else { return };
        if d.ghost.is_none() {
            if (x - d.x).hypot(y - d.y) < 6.0 {
                return;
            }
            let src = self.root.query_selector(&format!("[data-sq=\"{sel}\"] .pc")).ok().flatten();
            let ghost = src.as_ref().and_then(|s| s.clone_node_with_deep(true).ok()).and_then(|n| n.dyn_into::<HtmlElement>().ok());
            let (Some(src), Some(g)) = (src, ghost) else {
                self.drag = None;
                return;
            };
            let _ = g.class_list().add_1("ghost");
            let _ = g.class_list().remove_1("slide");
            let size = src.get_bounding_client_rect();
            let _ = g.style().set_property("width", &format!("{}px", size.width()));
            let _ = g.style().set_property("height", &format!("{}px", size.height()));
            if let Some(body) = doc().body() {
                let _ = body.append_child(&g);
            }
            set_style(&src, "opacity", "0.25");
            d.ghost = Some(g);
        }
        if let Some(g) = &d.ghost {
            let _ = g.style().set_property("left", &format!("{x}px"));
            let _ = g.style().set_property("top", &format!("{y}px"));
        }
    }
}

// ---------------------------------------------------------------- app state

#[derive(Clone, Copy)]
enum JobKind {
    Analysis,
    Cpu { token: u32 },
}

struct Job {
    fen: String,
    opts: SearchOpts,
    kind: JobKind,
}

#[derive(Clone)]
struct Judge {
    token: u32,
    fen_before: String,
    /// `None` when the move ended the game, so there is nothing to analyse afterwards.
    fen_after: Option<String>,
    m: Move,
    san: String,
    /// Number of half-moves played once this move was made.
    ply: usize,
}

/// A game that ended by resignation or agreement rather than on the board.
struct Ended {
    text: String,
    /// "1-0", "0-1" or "1/2-1/2".
    result: &'static str,
}

/// Something pasted into the save-and-load box that has been checked and can be loaded.
enum Loaded {
    Position(String),
    Game { start: String, moves: Vec<String>, result: Option<String> },
}

/// An action that waits for a yes or no.
enum Confirm {
    NewGame,
    NewGameAs(usize),
    StartFrom(String),
    Resign,
    Draw,
    Load(Loaded),
    /// Create an online game and its invitation link.
    Invite,
    /// The other player in an online game has offered a draw.
    AnswerDraw,
}

/// Work out whether `text` is a FEN position or a PGN game, and check that it can be played.
fn read_game_text(text: &str, scratch: &mut Engine) -> Result<Loaded, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("Paste a PGN game or a FEN position into the box first.".into());
    }
    if text.split_whitespace().next().is_some_and(|w| w.matches('/').count() == 7) {
        validate_fen(text)?;
        scratch.load(text);
        return Ok(Loaded::Position(scratch.fen()));
    }
    let pgn = engine::pgn::parse(text);
    let start = match &pgn.fen {
        Some(fen) => {
            validate_fen(fen).map_err(|why| format!("The starting position in this PGN is not valid. {why}"))?;
            fen.as_str()
        }
        None => START,
    };
    scratch.load(start);
    let start = scratch.fen();
    if pgn.moves.is_empty() {
        return match pgn.fen {
            Some(_) => Ok(Loaded::Position(start)),
            None => Err("That does not look like a PGN game or a FEN position.".into()),
        };
    }
    for san in &pgn.moves {
        if scratch.play_san(san).is_none() {
            let fen = scratch.fen();
            let number = fen.split(' ').nth(5).unwrap_or("?");
            let dots = if scratch.turn() == 1 { "…" } else { "." };
            return Err(format!("{number}{dots} {san} is not a legal move there, so nothing was loaded."));
        }
    }
    Ok(Loaded::Game { start, moves: pgn.moves, result: pgn.result })
}

struct Play {
    g: Engine,
    /// Position the game began from.
    start_fen: String,
    cpu: bool,
    level: usize,
    me: usize,
    coach: bool,
    /// Bumped whenever the game is reset or rewound, so stale engine replies are dropped.
    token: u32,
    thinking: bool,
    flipped: bool,
    hint: Option<SearchOut>,
    judge: Option<Judge>,
    eval_fen: Option<String>,
    hint_req: Option<(String, u32)>,
    /// When the computer was last asked for a move.
    asked_at: f64,
    ended: Option<Ended>,
    /// The coach's judgement of each half-move: 0 not rated, 1 best … 5 blunder.
    marks: Vec<u8>,
    /// Half-move being looked at while going back through the game; `None` is the live position.
    view: Option<usize>,
    /// Scratch board for showing earlier positions and checking pasted games.
    past: Engine,
    /// A draw offer waiting for the computer's evaluation: (position, token).
    draw_req: Option<(String, u32)>,
    confirm: Option<Confirm>,
    /// The chosen time control, an index into `TIME_CONTROLS`. Used from the next game on.
    tc: usize,
    /// This game's clocks, if it has any.
    clock: Option<Clock>,
    /// A move made while it was the other side's turn (from, to, promotion piece),
    /// to be played as soon as it is ours.
    premove: Option<(u8, u8, u8)>,
    /// Set while the board is showing a game played against a friend over the network.
    online: Option<Online>,
}

impl Play {
    fn status(&mut self) -> Status {
        match &self.ended {
            Some(e) => Status { over: true, mate: false, text: e.text.clone() },
            None => self.g.status(),
        }
    }

    /// "1-0", "0-1", "1/2-1/2", or "*" while the game is still going.
    fn result(&mut self) -> &'static str {
        if let Some(e) = &self.ended {
            return e.result;
        }
        let st = self.g.status();
        match (st.over, st.mate, self.g.turn()) {
            (false, _, _) => "*",
            (true, true, 1) => "1-0",
            (true, true, _) => "0-1",
            _ => "1/2-1/2",
        }
    }

    fn in_progress(&mut self) -> bool {
        let watching = self.online.as_ref().is_some_and(|o| o.me == 2);
        !watching && !self.g.history().is_empty() && !self.status().over
    }

    /// The side the person at this screen plays, when they play only one:
    /// against the computer or online. `None` with two players at one screen, or when watching.
    fn my_side(&self) -> Option<usize> {
        match &self.online {
            Some(o) => (o.me < 2).then_some(o.me),
            None => self.cpu.then_some(self.me),
        }
    }

    /// Whether the person at this screen may move a piece now.
    fn my_turn(&self) -> bool {
        match (&self.online, self.my_side()) {
            (Some(o), Some(me)) => o.joined && self.g.turn() == me,
            (Some(_), None) => false,
            (None, Some(me)) => self.g.turn() == me,
            (None, None) => true,
        }
    }
}

struct Learn {
    g: Engine,
    cur: usize,
    solved: bool,
}

struct Ply {
    m: Move,
    san: String,
    fen: String,
}

struct Openings {
    g: Engine,
    cur: usize,
    ply: usize,
    line: Vec<Ply>,
}

#[derive(Clone, Copy, PartialEq)]
enum PzState {
    Play,
    Wait,
    Done,
}

struct Puzzles {
    g: Engine,
    cur: usize,
    /// Moves left to deliver mate.
    left: i32,
    idx: usize,
    state: PzState,
    hint: Option<usize>,
    /// Invalidates a pending opponent reply when the puzzle changes.
    timer: u32,
}

struct App {
    boards: [BoardView; 4],
    view: usize,
    progress: Progress,
    settings: Settings,
    sound: Sound,
    /// Counts online games joined and left, to tell their replies apart.
    online_gen: u32,
    worker: Option<Worker>,
    seq: u32,
    jobs: HashMap<u32, Job>,
    /// Position -> analysis. `None` while the search is still running.
    cache: HashMap<String, Option<Option<SearchOut>>>,
    play: Play,
    learn: Learn,
    open: Openings,
    pz: Puzzles,
}

impl App {
    fn new() -> App {
        App {
            boards: [
                BoardView::new("play-board"),
                BoardView::new("learn-board"),
                BoardView::new("open-board"),
                BoardView::new("pz-board"),
            ],
            view: PLAY,
            progress: Progress::load(),
            settings: Settings::load(),
            sound: Sound::new(local_get(SOUND_KEY).as_deref() != Some("0")),
            online_gen: 0,
            worker: start_worker(),
            seq: 0,
            jobs: HashMap::new(),
            cache: HashMap::new(),
            play: Play {
                g: new_engine(),
                start_fen: START.to_string(),
                cpu: true,
                level: 2,
                me: 0,
                coach: true,
                token: 0,
                thinking: false,
                flipped: false,
                hint: None,
                judge: None,
                eval_fen: None,
                hint_req: None,
                asked_at: 0.0,
                ended: None,
                marks: Vec::new(),
                view: None,
                past: new_engine(),
                draw_req: None,
                confirm: None,
                tc: 0,
                clock: None,
                premove: None,
                online: None,
            },
            learn: Learn { g: new_engine(), cur: 0, solved: false },
            open: Openings { g: new_engine(), cur: 0, ply: 0, line: Vec::new() },
            pz: Puzzles { g: new_engine(), cur: 0, left: 0, idx: 0, state: PzState::Play, hint: None, timer: 0 },
        }
    }

    // ---- engine jobs: worker first, same-thread fallback ----

    fn request(&mut self, fen: String, opts: SearchOpts, kind: JobKind) {
        self.seq += 1;
        let id = self.seq;
        if let Some(w) = &self.worker {
            if w.post_message(&JsValue::from_str(&encode_request(id, &fen, opts))).is_ok() {
                self.jobs.insert(id, Job { fen, opts, kind });
                return;
            }
        }
        self.jobs.insert(id, Job { fen: fen.clone(), opts, kind });
        set_timeout(30, move || {
            let out = run_search(&fen, opts);
            with_app(|a| a.on_result(id, out));
        });
    }

    /// The worker could not start or crashed: finish its outstanding jobs on this thread.
    fn worker_failed(&mut self) {
        self.worker = None;
        for (&id, job) in &self.jobs {
            let (fen, opts) = (job.fen.clone(), job.opts);
            set_timeout(0, move || {
                let out = run_search(&fen, opts);
                with_app(|a| a.on_result(id, out));
            });
        }
    }

    fn on_result(&mut self, id: u32, out: Option<SearchOut>) {
        let Some(job) = self.jobs.remove(&id) else { return };
        match job.kind {
            JobKind::Analysis => {
                self.cache.insert(job.fen, Some(out));
                self.play_poll();
            }
            JobKind::Cpu { token } => self.play_cpu_reply(token, out),
        }
    }

    fn analyse(&mut self, fen: &str) {
        if !self.cache.contains_key(fen) {
            if self.cache.len() > 300 {
                self.cache.clear();
            }
            self.cache.insert(fen.to_string(), None);
            self.request(fen.to_string(), ANALYSIS, JobKind::Analysis);
        }
    }

    /// `Some(result)` once the analysis of `fen` has finished; requests it if nobody has yet.
    fn analysis(&mut self, fen: &str) -> Option<Option<SearchOut>> {
        self.analyse(fen);
        self.cache.get(fen).cloned().flatten()
    }

    // ---- board events, shared by all four boards ----

    fn get_moves(&mut self, w: usize, sq: usize) -> Vec<Move> {
        let from = |g: &mut Engine| g.legal().into_iter().filter(|m| m.from as usize == sq).collect();
        match w {
            PLAY => {
                let p = &mut self.play;
                if p.view.is_some() || p.status().over {
                    Vec::new()
                } else if p.my_turn() {
                    from(&mut p.g)
                } else {
                    // Not our turn: a piece may still be picked up to move in advance.
                    self.premove_moves(sq)
                }
            }
            LEARN => {
                let moves_wanted = matches!(&LESSONS[self.learn.cur].task, Some(t) if t.square.is_none());
                if self.learn.solved || !moves_wanted {
                    Vec::new()
                } else {
                    from(&mut self.learn.g)
                }
            }
            PUZZLE if self.pz.state == PzState::Play => from(&mut self.pz.g),
            _ => Vec::new(),
        }
    }

    fn on_move(&mut self, w: usize, m: Move) {
        match w {
            PLAY => self.play_on_move(m),
            LEARN => self.learn_on_move(m),
            PUZZLE => self.pz_on_move(m),
            _ => {}
        }
    }

    fn commit(&mut self, w: usize, sq: usize) {
        let b = &mut self.boards[w];
        let ms: Vec<Move> = b.targets.iter().copied().filter(|m| m.to as usize == sq).collect();
        if ms.len() > 1 {
            b.promo = Some(ms);
            b.render();
            return;
        }
        b.sel = None;
        b.targets.clear();
        if let Some(&m) = ms.first() {
            self.on_move(w, m);
        }
    }

    fn board_down(&mut self, w: usize, e: PointerEvent) {
        let Some(target) = e.target().and_then(|t| t.dyn_into::<Element>().ok()) else { return };
        let square = target
            .closest(".sq")
            .ok()
            .flatten()
            .and_then(|s| s.get_attribute("data-sq"))
            .and_then(|v| v.parse::<usize>().ok());
        self.boards[w].kbd = false;
        if e.button() == 2 {
            // Right button: the start of an arrow. It also calls off a move made in advance.
            self.boards[w].rdown = square;
            if w == PLAY && self.play.premove.take().is_some() {
                self.play_refresh();
            }
            e.prevent_default();
            return;
        }
        if e.button() != 0 {
            return;
        }
        if !self.boards[w].arrows.is_empty() {
            self.boards[w].arrows.clear();
            self.boards[w].render();
        }
        if self.boards[w].promo.is_some() {
            let pick = target
                .closest("[data-promo]")
                .ok()
                .flatten()
                .and_then(|b| b.get_attribute("data-promo"))
                .and_then(|v| v.parse::<u8>().ok());
            self.board_promo(w, pick);
            return;
        }
        let Some(sq) = square else { return };
        self.boards[w].cursor = sq;
        if self.board_press(w, sq) {
            self.boards[w].drag = Some(Drag { x: e.client_x() as f64, y: e.client_y() as f64, ghost: None });
            e.prevent_default();
        }
    }

    /// A promotion piece was chosen or, with `None`, the choice was abandoned.
    fn board_promo(&mut self, w: usize, pick: Option<u8>) {
        let Some(pm) = self.boards[w].promo.take() else { return };
        self.boards[w].sel = None;
        self.boards[w].targets.clear();
        match pick.and_then(|t| pm.iter().copied().find(|m| m.promo == t)) {
            Some(m) => self.on_move(w, m),
            None => self.boards[w].render(),
        }
    }

    /// A square was pressed, or chosen with the keyboard. True if that picked a piece up.
    fn board_press(&mut self, w: usize, sq: usize) -> bool {
        self.boards[w].dropped = false;
        if w == LEARN {
            self.learn_on_square(sq);
        }
        if self.boards[w].sel.is_some() && self.boards[w].targets.iter().any(|m| m.to as usize == sq) {
            self.commit(w, sq);
            return false;
        }
        // Touching the board anywhere else calls off a move made in advance.
        let called_off = w == PLAY && self.play.premove.take().is_some();
        let mv = self.get_moves(w, sq);
        let b = &mut self.boards[w];
        let picked = !mv.is_empty();
        if picked {
            b.sel = Some(sq);
            b.targets = mv;
            let file = (sq & 7) as f32;
            let at = At { piece: b.pos[sq] & 7, file: if b.flipped { 7.0 - file } else { file }, force: 0 };
            self.sound.play_at(Sfx::Select, 0.0, at);
        } else {
            b.sel = None;
            b.targets.clear();
        }
        if called_off {
            self.play_refresh();
        } else {
            self.boards[w].render();
        }
        picked
    }

    fn pointer_end(&mut self, e: PointerEvent, released: bool) {
        // The right button coming up finishes an arrow.
        for b in self.boards.iter_mut() {
            let Some(from) = b.rdown.take() else { continue };
            let under = if released { doc().element_from_point(e.client_x() as f32, e.client_y() as f32) } else { None };
            let to = under
                .and_then(|x| x.closest(".sq").ok().flatten())
                .filter(|s| b.root.contains(Some(s)))
                .and_then(|s| s.get_attribute("data-sq"))
                .and_then(|v| v.parse::<usize>().ok());
            if let Some(to) = to {
                // Drawing the same arrow again rubs it out.
                match b.arrows.iter().position(|a| *a == (from, to)) {
                    Some(i) => drop(b.arrows.remove(i)),
                    None => b.arrows.push((from, to)),
                }
                b.render();
            }
        }
        for w in 0..self.boards.len() {
            let Some(d) = self.boards[w].drag.take() else { continue };
            let Some(ghost) = d.ghost else { continue };
            ghost.remove();
            let b = &self.boards[w];
            let drop = if released { doc().element_from_point(e.client_x() as f32, e.client_y() as f32) } else { None };
            let sq = drop
                .and_then(|x| x.closest(".sq").ok().flatten())
                .filter(|s| b.root.contains(Some(s)))
                .and_then(|s| s.get_attribute("data-sq"))
                .and_then(|v| v.parse::<usize>().ok())
                .filter(|sq| b.targets.iter().any(|m| m.to as usize == *sq));
            match sq {
                Some(sq) => {
                    self.boards[w].dropped = true;
                    self.commit(w, sq);
                }
                None => self.boards[w].render(),
            }
        }
    }

    /// Slide the pieces of a move that was just played on `w`'s game, with its sound.
    fn show_move(&mut self, w: usize, m: Move, sfx: (Sfx, u8)) -> f64 {
        let delay = self.boards[w].slide(move_pairs(m));
        let at = self.sound_at(w, m, sfx.1);
        self.sound.play_at(sfx.0, delay, at);
        delay
    }

    /// Which piece a move's sound belongs to, where on screen it lands, and how hard.
    fn sound_at(&self, w: usize, m: Move, force: u8) -> At {
        let file = (m.to & 7) as f32;
        At { piece: m.piece & 7, file: if self.boards[w].flipped { 7.0 - file } else { file }, force }
    }

    // ================================================================ PLAY

    fn play_on_move(&mut self, m: Move) {
        if !self.play.my_turn() {
            // Not our turn yet: remember the move and play it the moment it becomes possible.
            self.boards[PLAY].dropped = false;
            self.play.premove = Some((m.from, m.to, m.promo));
            self.play_refresh();
            return;
        }
        let mover = self.play.g.turn();
        let ply = self.play.g.history().len();
        let fen_before = self.play.g.fen();
        let san = self.play.g.play(m);
        let fen_after = self.play.g.fen();
        let over = self.play.g.status();
        self.play.hint = None;
        self.play.draw_req = None;
        self.play.premove = None;
        let sfx = sfx_for(&mut self.play.g, m);
        self.show_move(PLAY, m, sfx);
        self.play_announce(mover, &san);
        if self.play.online.is_some() {
            // The server checks the move and passes it on; there is no coach in a game against a person.
            self.play_refresh();
            self.play_prime();
            self.online_send_move(san, ply);
            return;
        }
        self.clock_moved(mover);
        self.play_refresh();
        if self.play.coach {
            self.play_judge(fen_before, m, san, fen_after, &over);
        }
        self.play_prime();
        self.play_cpu_move();
    }

    fn play_judge(&mut self, fen_before: String, m: Move, san: String, fen_after: String, over: &Status) {
        let ply = self.play.g.history().len();
        if over.mate {
            verdict("play-verdict", "best", "Mate", &format!("{} Checkmate. Well played.", san_tag(&san)));
            self.play_mark(ply, 1);
            return;
        }
        verdict("play-verdict", "", "Coach", &format!("Checking {}…", san_tag(&san)));
        self.play.judge = Some(Judge {
            token: self.play.token,
            fen_before,
            fen_after: if over.over { None } else { Some(fen_after) },
            m,
            san,
            ply,
        });
        self.play_poll();
    }

    /// Record the coach's judgement of the move that led to `ply` (1 best … 5 blunder).
    fn play_mark(&mut self, ply: usize, class: u8) {
        if let Some(slot) = ply.checked_sub(1).and_then(|i| self.play.marks.get_mut(i)) {
            *slot = class;
        }
        self.play_render_moves();
        self.play_summary();
        self.play_save();
    }

    /// Run after anything that may have completed an analysis: settles the coach
    /// verdict, the evaluation bar, the hint and a draw offer if their inputs are ready.
    fn play_poll(&mut self) {
        if let Some(j) = self.play.judge.clone() {
            if j.token != self.play.token {
                self.play.judge = None;
            } else {
                let before = self.analysis(&j.fen_before);
                let after = match &j.fen_after {
                    Some(f) => self.analysis(f),
                    None => Some(None),
                };
                if let (Some(a), Some(b)) = (before, after) {
                    self.play.judge = None;
                    if let Some(a) = a {
                        let class = show_judgement(&j, &a, b.as_ref());
                        self.play_mark(j.ply, class);
                    }
                }
            }
        }
        if let Some(fen) = self.play.eval_fen.clone() {
            if let Some(a) = self.analysis(&fen) {
                self.play.eval_fen = None;
                if let (Some(a), true) = (a, self.play_shown_fen() == fen) {
                    let white_to_move = fen.split(' ').nth(1) != Some("b");
                    self.set_eval(Some(if white_to_move { a.score } else { -a.score }));
                }
            }
        }
        if let Some((fen, token)) = self.play.hint_req.clone() {
            if let Some(a) = self.analysis(&fen) {
                self.play.hint_req = None;
                let current = token == self.play.token && self.play.view.is_none() && self.play.g.fen() == fen;
                if let (Some(a), true) = (a, current) {
                    let text = format!("Consider {}. The squares are marked on the board.", san_tag(&a.san));
                    verdict("play-verdict", "ok", "Hint", &text);
                    self.play.hint = Some(a);
                    self.play_refresh();
                }
            }
        }
        if let Some((fen, token)) = self.play.draw_req.clone() {
            if let Some(a) = self.analysis(&fen) {
                self.play.draw_req = None;
                if token == self.play.token && self.play.g.fen() == fen && !self.play.status().over {
                    // The score is for the side to move, which is the player who offered.
                    let computer = -a.map(|a| a.score).unwrap_or(0);
                    let long_game = self.play.g.history().len() >= 60;
                    if computer <= -80 || (long_game && computer <= 40) {
                        self.play_end("Draw agreed.", "1/2-1/2");
                        verdict("play-verdict", "ok", "Draw", "The computer accepts your offer.");
                    } else if computer > 150 {
                        verdict("play-verdict", "no", "Draw", "The computer declines. It likes its position.");
                    } else {
                        verdict("play-verdict", "no", "Draw", "The computer declines and wants to play on.");
                    }
                }
            }
        }
    }

    /// FEN of the position on the board: the live one, or an earlier one being looked at.
    fn play_shown_fen(&self) -> String {
        match self.play.view {
            Some(_) => self.play.past.fen(),
            None => self.play.g.fen(),
        }
    }

    /// Ask for an evaluation of the position on the board to drive the eval bar.
    fn play_prime(&mut self) {
        if self.play.view.is_none() && self.play.status().over {
            self.set_eval(None);
            return;
        }
        if self.play.online.is_some() && !self.play.status().over {
            // No engine's opinion while a game against a person is under way.
            self.play.eval_fen = None;
            if let Some(fill) = el("evalbar").first_element_child() {
                set_style(&fill, "height", "50%");
            }
            set_text("play-eval", "");
            return;
        }
        self.play.eval_fen = Some(self.play_shown_fen());
        self.play_poll();
    }

    /// `Some(centipawns)` from White's point of view, or `None` when the game is over.
    fn set_eval(&mut self, white: Option<i32>) {
        let bar = el("evalbar");
        let _ = bar.class_list().toggle_with_force("flip", self.play.flipped);
        let Some(fill) = bar.first_element_child() else { return };
        let (height, label) = match white {
            None => match self.play.result() {
                "1-0" => ("100%".to_string(), "1-0".to_string()),
                "0-1" => ("0%".to_string(), "0-1".to_string()),
                _ => ("50%".to_string(), "½-½".to_string()),
            },
            Some(w) => {
                let label = if w.abs() > 90_000 {
                    String::from(if w > 0 { "+M" } else { "-M" })
                } else {
                    format!("{}{:.1}", if w > 0 { "+" } else { "" }, w as f64 / 100.0)
                };
                (format!("{:.1}%", 50.0 + 50.0 * (w as f64 / 500.0).tanh()), label)
            }
        };
        set_style(&fill, "height", &height);
        set_text("play-eval", &label);
    }

    fn play_cpu_move(&mut self) {
        let p = &mut self.play;
        if !p.cpu || p.g.turn() == p.me || p.status().over {
            return;
        }
        p.thinking = true;
        p.asked_at = now();
        let (fen, token, mut opts, side) = (p.g.fen(), p.token, LEVELS[p.level], p.g.turn());
        // On the clock, the computer budgets its time like anyone else.
        if let Some(left) = self.clock_left(side) {
            opts.time_ms = opts.time_ms.min((left / 25.0).max(40.0));
        }
        self.play_refresh();
        // The worker answers in order, so the evaluation lands before the move does.
        self.analyse(&fen);
        self.request(fen, opts, JobKind::Cpu { token });
    }

    /// The engine has chosen. Hold the move back until the player's own piece has
    /// landed and a beat has passed, so the reply never feels instant.
    fn play_cpu_reply(&mut self, token: u32, out: Option<SearchOut>) {
        if token != self.play.token {
            return;
        }
        // Short of time, the computer does not dawdle either.
        let side = self.play.g.turn();
        let pause = self.clock_left(side).map_or(min_reply_ms(), |left| min_reply_ms().min(left / 30.0));
        let wait = pause - (now() - self.play.asked_at);
        if wait > 10.0 {
            set_timeout(wait as i32, move || with_app(|a| a.play_cpu_apply(token, out)));
        } else {
            self.play_cpu_apply(token, out);
        }
    }

    fn play_cpu_apply(&mut self, token: u32, out: Option<SearchOut>) {
        if token != self.play.token {
            return;
        }
        self.play.thinking = false;
        let mut moved = false;
        if let Some(r) = out {
            let g = &mut self.play.g;
            if let Some(m) = g.legal().into_iter().find(|x| x.from == r.from && x.to == r.to && x.promo == r.promo) {
                let mover = g.turn();
                let san = g.play(m);
                // A new move is worth seeing: leave any earlier position being looked at.
                self.play.view = None;
                // Squares offered for a move in advance were worked out before this move.
                self.boards[PLAY].deselect();
                let sfx = sfx_for(&mut self.play.g, m);
                self.show_move(PLAY, m, sfx);
                self.clock_moved(mover);
                self.play_announce(mover, &san);
                moved = true;
            }
        }
        self.play_refresh();
        self.play_prime();
        if moved {
            self.play_run_premove();
        }
    }

    fn side_row(&self, id: &str, color: usize) {
        const START_COUNT: [i32; 6] = [0, 8, 2, 2, 2, 1];
        const POINTS: [i32; 7] = [0, 1, 3, 3, 5, 9, 0];
        let mut have = [[0i32; 7]; 2];
        let mut mat = [0i32; 2];
        for &p in self.boards[PLAY].pos.iter().filter(|p| **p != 0) {
            have[(p >> 3) as usize][(p & 7) as usize] += 1;
            mat[(p >> 3) as usize] += POINTS[(p & 7) as usize];
        }
        let mut caps = String::new();
        for t in [5usize, 4, 3, 2, 1] {
            let n = (START_COUNT[t] - have[color ^ 1][t]).max(0);
            let taken = format!("<i class=\"{}\"></i>", piece_class(t as u8 | ((color as u8 ^ 1) << 3)));
            caps.push_str(&taken.repeat(n as usize));
        }
        let lead = mat[color] - mat[color ^ 1];
        let who = self.play_names()[color];
        let lead = if lead > 0 { format!("<span class=\"lead\">+{lead}</span>") } else { String::new() };
        let clock = self.clock_html(color);
        set_html(
            id,
            &format!(
                "<span class=\"dot {}\"></span><b>{who}</b><span class=\"cap\">{caps}</span>{lead}{clock}",
                if color == 1 { "b" } else { "w" }
            ),
        );
    }

    /// What to call White and Black on screen.
    fn play_names(&self) -> [&'static str; 2] {
        if let Some(o) = &self.play.online {
            return match o.me {
                0 => ["You", "Friend"],
                1 => ["Friend", "You"],
                _ => ["White", "Black"],
            };
        }
        match (self.play.cpu, self.play.me) {
            (true, 0) => ["You", "Computer"],
            (true, _) => ["Computer", "You"],
            _ => ["White", "Black"],
        }
    }

    /// "You resigned. The computer wins." and the like: `loser` did something that lost the game.
    fn play_loss_text(&self, loser: usize, did: &str) -> String {
        let full = |who: &'static str| match who {
            "Computer" => "The computer",
            "Friend" => "Your friend",
            other => other,
        };
        let names = self.play_names();
        let (lost, won) = (full(names[loser]), full(names[loser ^ 1]));
        if won == "You" {
            format!("{lost} {did}. You win.")
        } else {
            format!("{lost} {did}. {won} wins.")
        }
    }

    /// Moves `sq`'s piece could make if it were our turn, to be made in advance.
    fn premove_moves(&mut self, sq: usize) -> Vec<Move> {
        let p = &mut self.play;
        let Some(me) = p.my_side() else { return Vec::new() };
        if p.online.as_ref().is_some_and(|o| !o.joined) {
            return Vec::new();
        }
        // The same position with the move handed to us, on the scratch board.
        let fen = p.g.fen();
        let mut f: Vec<&str> = fen.split(' ').collect();
        if f.len() < 4 {
            return Vec::new();
        }
        f[1] = if me == 0 { "w" } else { "b" };
        f[3] = "-";
        p.past.load(&f.join(" "));
        p.past.legal().into_iter().filter(|m| m.from as usize == sq && m.cap & 7 != engine::K).collect()
    }

    /// The other side has just moved: play the move that was made in advance, if there is one.
    fn play_run_premove(&mut self) {
        let Some(pre) = self.play.premove else { return };
        let token = self.play.token;
        // Let the other side's piece land first.
        set_timeout(move_ms() as i32 + 80, move || with_app(|a| a.play_premove_now(token, pre)));
    }

    fn play_premove_now(&mut self, token: u32, pre: (u8, u8, u8)) {
        if token != self.play.token || self.play.premove != Some(pre) {
            return;
        }
        self.play.premove = None;
        let p = &mut self.play;
        let ready = p.view.is_none() && !p.status().over && p.my_turn();
        // It is played only if the position that arose still allows it.
        let m = if ready { p.g.legal().into_iter().find(|m| (m.from, m.to, m.promo) == pre) } else { None };
        match m {
            Some(m) => {
                self.boards[PLAY].deselect();
                self.play_on_move(m);
            }
            None => self.play_refresh(),
        }
    }

    /// Number of the first move, and 1 if Black made it (games begun from a set-up position).
    fn play_numbering(&self) -> (usize, usize) {
        let f: Vec<&str> = self.play.start_fen.split_whitespace().collect();
        (f.get(5).and_then(|n| n.parse().ok()).unwrap_or(1), usize::from(f.get(1) == Some(&"b")))
    }

    /// "12. Nf3" or "12… Nf6" for the move that led to `ply`.
    fn play_move_label(&self, ply: usize) -> String {
        let (first, off) = self.play_numbering();
        let half = ply - 1 + off;
        let san = self.play.g.history().get(ply - 1).map(|h| h.san.as_str()).unwrap_or("");
        format!("{}{} {san}", first + half / 2, if half % 2 == 0 { "." } else { "…" })
    }

    fn play_refresh(&mut self) {
        let p = &mut self.play;
        let st = p.status();
        let total = p.g.history().len();
        p.marks.resize(total, 0);
        if p.view.is_some_and(|k| k >= total) {
            p.view = None;
        }
        let b = &mut self.boards[PLAY];
        b.flipped = p.flipped;
        match p.view {
            Some(k) => {
                // Rebuild the earlier position on a scratch board; the game itself is untouched.
                p.past.load(&p.start_fen);
                for h in &p.g.history()[..k] {
                    p.past.play(h.m);
                }
                b.sync(&p.past);
                b.last = k.checked_sub(1).map(|i| p.g.history()[i].m).map(|m| (m.from as usize, m.to as usize));
                b.marks.clear();
            }
            None => {
                b.sync(&p.g);
                b.last = p.g.history().last().map(|h| (h.m.from as usize, h.m.to as usize));
                b.marks = p.hint.iter().flat_map(|h| [(h.from as usize, "hint"), (h.to as usize, "hint")]).collect();
                if let Some((from, to, _)) = p.premove {
                    b.marks.extend([(from as usize, "pre"), (to as usize, "pre")]);
                }
            }
        }
        b.render();

        let live = p.view.is_none();
        let my_turn = p.my_turn();
        let shown = p.view.unwrap_or(total);
        let online = p.online.is_some();
        // In an online game only its two players, once both are there, can end it.
        let seated = p.online.as_ref().is_none_or(|o| o.joined && o.me < 2);
        set_disabled("btn-undo", online || (total == 0 && p.ended.is_none()));
        set_disabled("btn-hint", online || !live || st.over || p.thinking || !my_turn);
        set_disabled("btn-resign", st.over || !seated);
        set_disabled("btn-draw", st.over || p.thinking || !seated || (p.cpu && !my_turn));
        set_text("btn-draw", if p.cpu || online { "Offer draw" } else { "Agree draw" });
        set_disabled("sel-mode", online);
        set_disabled("sel-level", online || !p.cpu);
        set_disabled("sel-color", online || !p.cpu);
        set_disabled("sel-time", online);
        set_disabled("play-san", st.over || !live);
        set_disabled("btn-san", st.over || !live);
        for id in ["play-first", "play-prev"] {
            set_disabled(id, shown == 0);
        }
        for id in ["play-next", "play-last"] {
            set_disabled(id, shown == total);
        }

        let side = if p.g.turn() == 1 { "Black" } else { "White" };
        let (thinking, in_check, flipped) = (p.thinking, p.g.in_check(), p.flipped);
        let waiting = p.online.as_ref().is_some_and(|o| !o.joined);
        let mine = match (&p.online, p.my_side()) {
            (Some(_), Some(me)) => Some(p.g.turn() == me),
            _ => None,
        };
        let check = if in_check { " · check" } else { "" };
        let status = match self.play.view {
            Some(0) => String::from("Looking back at the start"),
            Some(k) => format!("Looking back at {}", self.play_move_label(k)),
            None if st.over => st.text.clone(),
            None if thinking => String::from("Computer is thinking…"),
            None if waiting => String::from("Waiting for your friend to join…"),
            None if mine == Some(true) => format!("Your move{check}"),
            None if mine == Some(false) => format!("Your friend's move{check}"),
            None => format!("{side} to move{check}"),
        };
        set_text("play-status", &status);
        self.side_row("play-top", if flipped { 0 } else { 1 });
        self.side_row("play-bot", if flipped { 1 } else { 0 });
        if live && st.over {
            self.set_eval(None);
        }
        self.play_render_moves();
        self.play_summary();
        self.play_save();
    }

    fn play_render_moves(&self) {
        let p = &self.play;
        let h = p.g.history();
        let shown = p.view.unwrap_or(h.len());
        let (first, off) = self.play_numbering();
        // `ply` counts from 1; 0 stands for White's missing move when Black began.
        let cell = |ply: usize| -> String {
            let Some((i, played)) = ply.checked_sub(1).and_then(|i| h.get(i).map(|x| (i, x))) else {
                return String::from(if ply == 0 { "<span class=\"muted\">…</span>" } else { "<span></span>" });
            };
            let nag = match p.marks.get(i) {
                Some(3) => "<i class=\"nag inacc\" title=\"Inaccuracy\">?!</i>",
                Some(4) => "<i class=\"nag mistake\" title=\"Mistake\">?</i>",
                Some(5) => "<i class=\"nag blunder\" title=\"Blunder\">??</i>",
                _ => "",
            };
            let cur = if ply == shown { " cur" } else { "" };
            format!("<span class=\"ply{cur}\" data-p=\"{ply}\">{}{nag}</span>", played.san)
        };
        let mut html = String::new();
        for j in (0..h.len() + off).step_by(2) {
            let white = if j < off { 0 } else { j + 1 - off };
            html.push_str(&format!("<span class=\"n\">{}.</span>{}{}", first + j / 2, cell(white), cell(j + 2 - off)));
        }
        if html.is_empty() {
            html.push_str("<span class=\"n\"></span><span class=\"muted\" style=\"grid-column:2/4;font-family:var(--body)\">No moves yet.</span>");
        }
        let moves = el("play-moves");
        moves.set_inner_html(&html);
        // Keep the move being shown in sight.
        let cur = moves.query_selector(".cur").ok().flatten().and_then(|c| c.dyn_into::<HtmlElement>().ok());
        moves.set_scroll_top(cur.map(|c| c.offset_top() - moves.client_height() / 2).unwrap_or(0));
    }

    /// After the game: the result and how the coach rated each side's moves.
    fn play_summary(&mut self) {
        let st = self.play.status();
        set_hidden("play-summary", !st.over);
        if !st.over {
            return;
        }
        let (_, off) = self.play_numbering();
        let mut counts = [[0u32; 6]; 2];
        for (i, &m) in self.play.marks.iter().enumerate() {
            counts[(i + off) % 2][m as usize] += 1;
        }
        let names = self.play_names();
        let mut rows = String::new();
        for side in 0..2 {
            if counts[side][1..].iter().any(|n| *n > 0) {
                let cells: String = counts[side][1..].iter().map(|n| format!("<td>{n}</td>")).collect();
                rows.push_str(&format!("<tr><th>{}</th>{cells}</tr>", names[side]));
            }
        }
        let table = if rows.is_empty() {
            let why = if self.play.coach {
                "The coach rates moves as they are played, so this game has no breakdown."
            } else {
                "No moves were rated. Turn the coach on to get a breakdown next time."
            };
            format!("<span class=\"small muted\">{why}</span>")
        } else {
            format!("<table><tr><th></th><th>Best</th><th>Good</th><th title=\"Inaccuracies\">?!</th><th title=\"Mistakes\">?</th><th title=\"Blunders\">??</th></tr>{rows}</table>")
        };
        let moves = self.play.g.history().len().div_ceil(2);
        set_html(
            "play-summary",
            &format!(
                "<b>{}</b><span class=\"small muted\">{moves} move{} played. Click a move or use the arrows to go back through the game.</span>{table}",
                st.text,
                if moves == 1 { "" } else { "s" }
            ),
        );
    }

    // ---- stepping back through the game ----

    fn play_go(&mut self, ply: isize) {
        let total = self.play.g.history().len();
        let old = self.play.view.unwrap_or(total);
        let new = ply.clamp(0, total as isize) as usize;
        if new == old {
            return;
        }
        // Single steps slide the piece; jumps just show the position.
        let step = if new == old + 1 {
            Some((self.play.g.history()[new - 1].m, true))
        } else if new + 1 == old {
            Some((self.play.g.history()[old - 1].m, false))
        } else {
            None
        };
        self.boards[PLAY].clear();
        self.boards[PLAY].slide(match step {
            Some((m, true)) => move_pairs(m),
            Some((m, false)) => back_pairs(m),
            None => Vec::new(),
        });
        self.play.view = if new == total { None } else { Some(new) };
        self.play_refresh();
        if let Some((m, forward)) = step {
            let shown = if self.play.view.is_some() { &mut self.play.past } else { &mut self.play.g };
            let sfx = if forward { sfx_for(shown, m) } else { (Sfx::Move, 0) };
            let at = self.sound_at(PLAY, m, sfx.1);
            self.sound.play_at(sfx.0, move_ms(), at);
        }
        self.play_prime();
    }

    fn play_step(&mut self, by: isize) {
        let shown = self.play.view.unwrap_or(self.play.g.history().len());
        self.play_go(shown as isize + by);
    }

    // ---- starting, ending and undoing ----

    fn play_new_game(&mut self, fen: Option<&str>) {
        let p = &mut self.play;
        p.token += 1;
        p.thinking = false;
        p.hint = None;
        p.judge = None;
        p.draw_req = None;
        p.ended = None;
        p.view = None;
        p.premove = None;
        p.marks.clear();
        p.start_fen = fen.unwrap_or(START).to_string();
        p.g.load(&p.start_fen);
        p.flipped = p.my_side() == Some(1);
        p.confirm = None;
        set_hidden("play-confirm", true);
        set_text("time-note", "");
        self.clock_reset();
        self.boards[PLAY].clear();
        // The keyboard cursor starts on the player's own king's pawn.
        self.boards[PLAY].cursor = if self.play.flipped { 12 } else { 52 };
        verdict("play-verdict", "", "Coach", COACH_INTRO);
        self.play_refresh();
        self.play_prime();
        self.play_cpu_move();
    }

    /// End the game by agreement or resignation rather than on the board.
    fn play_end(&mut self, text: &str, result: &'static str) {
        let p = &mut self.play;
        p.ended = Some(Ended { text: text.to_string(), result });
        p.token += 1;
        p.thinking = false;
        p.hint = None;
        p.judge = None;
        p.draw_req = None;
        p.premove = None;
        self.clock_stop();
        self.play_refresh();
        self.play_prime();
    }

    fn play_resign(&mut self) {
        if self.play.status().over {
            return;
        }
        if self.play.online.is_some() {
            // The server ends the game and tells both players.
            self.online_act("resign");
            return;
        }
        let loser = if self.play.cpu { self.play.me } else { self.play.g.turn() };
        let text = self.play_loss_text(loser, "resigned");
        // Resigning over the board: the king is laid on its side.
        self.sound.play(Sfx::Topple, 0.0);
        verdict("play-verdict", "", "Coach", "Game over. Go back through the moves to see where it turned.");
        self.play_end(&text, if loser == 0 { "0-1" } else { "1-0" });
    }

    fn play_offer_draw(&mut self) {
        if self.play.status().over {
            return;
        }
        if self.play.online.is_some() {
            verdict("play-verdict", "", "Draw", "You offer a draw. Waiting for your friend's answer.");
            self.online_act("offer");
            return;
        }
        if !self.play.cpu {
            self.play_request(Confirm::Draw);
            return;
        }
        verdict("play-verdict", "", "Draw", "You offer a draw. The computer is considering it…");
        self.play.draw_req = Some((self.play.g.fen(), self.play.token));
        self.play_poll();
    }

    fn play_undo(&mut self) {
        if self.play.online.is_some() {
            return;
        }
        let p = &mut self.play;
        p.view = None;
        p.draw_req = None;
        p.premove = None;
        if p.ended.take().is_some() {
            // Undo after a resignation or an agreed draw puts the game back on, nothing more.
            p.token += 1;
            verdict("play-verdict", "", "Coach", "The game is on again.");
            self.clock_rewind();
            self.play_refresh();
            self.play_prime();
            self.play_cpu_move();
            return;
        }
        if p.g.history().is_empty() {
            return;
        }
        p.token += 1;
        p.thinking = false;
        p.hint = None;
        let mut back: Vec<(usize, usize)> = p.g.takeback().map(|h| back_pairs(h.m)).unwrap_or_default();
        // Against the computer, also take back its reply so it is the player's turn again.
        if p.cpu && p.g.turn() != p.me && !p.g.history().is_empty() {
            back.extend(p.g.takeback().map(|h| back_pairs(h.m)).unwrap_or_default());
        }
        self.boards[PLAY].clear();
        self.boards[PLAY].slide(back);
        self.sound.play(Sfx::Move, move_ms());
        verdict("play-verdict", "", "Coach", "Move taken back. Try something else.");
        self.clock_rewind();
        self.play_refresh();
        self.play_prime();
        self.play_cpu_move();
    }

    fn play_hint(&mut self) {
        if self.play.view.is_some() || self.play.online.is_some() {
            return;
        }
        verdict("play-verdict", "", "Hint", "Thinking…");
        self.play.hint_req = Some((self.play.g.fen(), self.play.token));
        self.play_poll();
    }

    /// Start a game against the computer from an opening position.
    fn play_start_from(&mut self, fen: &str) {
        self.play.cpu = true;
        self.play.me = if fen.split(' ').nth(1) == Some("b") { 1 } else { 0 };
        self.play_sync_controls();
        self.play_new_game(Some(fen));
    }

    /// Make the setup controls show the current settings.
    fn play_sync_controls(&self) {
        let p = &self.play;
        el("sel-mode").unchecked_into::<HtmlSelectElement>().set_value(if p.cpu { "cpu" } else { "human" });
        el("sel-level").unchecked_into::<HtmlSelectElement>().set_value(&p.level.to_string());
        el("sel-color").unchecked_into::<HtmlSelectElement>().set_value(&p.me.to_string());
        el("chk-coach").unchecked_into::<HtmlInputElement>().set_checked(p.coach);
        el("sel-time").unchecked_into::<HtmlSelectElement>().set_value(&p.tc.to_string());
    }

    // ---- "are you sure?" ----

    /// Do `c`, asking first if it would throw away a game in progress or end one.
    fn play_request(&mut self, c: Confirm) {
        let ask = match c {
            Confirm::Resign | Confirm::Draw | Confirm::AnswerDraw => true,
            _ => self.play.in_progress(),
        };
        if !ask {
            self.play_do(c);
            return;
        }
        let (text, yes, no) = match c {
            Confirm::Resign => ("Resign this game?", "Resign", "Keep playing"),
            Confirm::Draw => ("Agree a draw? Both players should say yes.", "Agree draw", "Keep playing"),
            Confirm::AnswerDraw => ("Your friend offers a draw.", "Accept draw", "Decline"),
            _ if self.play.online.is_some() => ("Leave this online game? That resigns it.", "Leave game", "Keep playing"),
            _ => ("This game is not finished. Abandon it?", "Abandon game", "Keep playing"),
        };
        self.play_ask(c, text, yes, no);
    }

    /// Put a question in the bar under the buttons; `play_answer` gets the reply.
    fn play_ask(&mut self, c: Confirm, text: &str, yes: &str, no: &str) {
        set_text("confirm-text", text);
        set_text("confirm-yes", yes);
        set_text("confirm-no", no);
        set_hidden("play-confirm", false);
        self.play.confirm = Some(c);
    }

    fn play_answer(&mut self, yes: bool) {
        set_hidden("play-confirm", true);
        let Some(c) = self.play.confirm.take() else { return };
        if yes {
            self.play_do(c);
        } else if matches!(c, Confirm::AnswerDraw) {
            self.online_act("decline");
        } else {
            // A colour picked in the dropdown but not confirmed must not stay selected.
            self.play_sync_controls();
        }
    }

    fn play_do(&mut self, c: Confirm) {
        // Anything that starts a different game ends an online one.
        if !matches!(c, Confirm::Resign | Confirm::Draw | Confirm::AnswerDraw | Confirm::Invite) {
            self.online_leave();
        }
        match c {
            Confirm::Invite => self.online_invite(),
            Confirm::AnswerDraw => self.online_act("accept"),
            Confirm::NewGame => {
                self.sound.play(Sfx::Setup, 0.0);
                self.play_new_game(None);
            }
            Confirm::NewGameAs(me) => {
                self.play.me = me;
                self.sound.play(Sfx::Setup, 0.0);
                self.play_new_game(None);
            }
            Confirm::StartFrom(fen) => {
                self.sound.play(Sfx::Setup, 0.0);
                self.play_start_from(&fen);
            }
            Confirm::Resign => self.play_resign(),
            Confirm::Draw => {
                if !self.play.status().over {
                    self.play_end("Draw agreed.", "1/2-1/2");
                }
            }
            Confirm::Load(loaded) => self.play_load(loaded),
        }
    }

    // ---- keeping the game across page loads ----

    fn play_save(&self) {
        let p = &self.play;
        let moves: Vec<&str> = p.g.history().iter().map(|h| h.san.as_str()).collect();
        let marks: String = p.marks.iter().map(|m| char::from(b'0' + (*m).min(5))).collect();
        let (end_text, end_result) = p.ended.as_ref().map(|e| (e.text.as_str(), e.result)).unwrap_or(("", ""));
        let clock = match (self.clock_left(0), self.clock_left(1)) {
            (Some(w), Some(b)) => format!("[{w:.0},{b:.0}]"),
            _ => String::from("null"),
        };
        // An online game is kept by the server; all that is needed here is the way back to it.
        let online = match &p.online {
            Some(o) => format!("{{\"id\":{},\"seat\":{},\"me\":{}}}", json_str(&o.id), json_str(&o.seat), o.me),
            None => String::from("null"),
        };
        let json = format!(
            "{{\"start\":{},\"moves\":{},\"marks\":{},\"cpu\":{},\"level\":{},\"me\":{},\"coach\":{},\"flipped\":{},\"endText\":{},\"endResult\":{},\"tc\":{},\"clock\":{clock},\"online\":{online}}}",
            json_str(&p.start_fen),
            json_str(&moves.join(" ")),
            json_str(&marks),
            p.online.as_ref().map_or(p.cpu, |o| o.was_cpu),
            p.level,
            p.me,
            p.coach,
            p.flipped,
            json_str(end_text),
            json_str(end_result),
            p.tc,
        );
        if let Ok(Some(s)) = window().local_storage() {
            let _ = s.set_item(GAME_KEY, &json);
        }
    }

    /// Pick up the game that was on the board when the page was last open.
    fn play_restore(&mut self) -> bool {
        let Some(v) = local_get(GAME_KEY).and_then(|raw| js_sys::JSON::parse(&raw).ok()) else { return false };
        let get = |k: &str| js_sys::Reflect::get(&v, &JsValue::from_str(k)).unwrap_or(JsValue::UNDEFINED);
        let text = |k: &str| get(k).as_string().unwrap_or_default();
        let start = text("start");
        if validate_fen(&start).is_err() {
            return false;
        }
        let p = &mut self.play;
        p.cpu = get("cpu").as_bool().unwrap_or(true);
        p.level = (get("level").as_f64().unwrap_or(2.0) as usize).min(LEVELS.len() - 1);
        p.me = get("me").as_f64().unwrap_or(0.0) as usize & 1;
        p.coach = get("coach").as_bool().unwrap_or(true);
        p.tc = (get("tc").as_f64().unwrap_or(0.0) as usize).min(TIME_CONTROLS.len() - 1);
        self.play_sync_controls();
        // An online game lives on the server: go back to it rather than to a saved position.
        let online = get("online");
        let part = |k: &str| js_sys::Reflect::get(&online, &JsValue::from_str(k)).unwrap_or(JsValue::UNDEFINED);
        if online.is_object() {
            if let (Some(id), Some(seat)) = (part("id").as_string(), part("seat").as_string()) {
                let me = part("me").as_f64().unwrap_or(2.0) as usize;
                let was_cpu = self.play.cpu;
                self.online_attach(id, seat, me, was_cpu);
                return true;
            }
        }
        self.play_new_game(Some(&start));
        let p = &mut self.play;
        p.flipped = get("flipped").as_bool().unwrap_or(p.flipped);
        // New-game set the computer thinking about the first move; the saved game replaces that.
        p.token += 1;
        p.thinking = false;
        for san in text("moves").split_whitespace() {
            if p.g.play_san(san).is_none() {
                break;
            }
        }
        p.marks = text("marks").bytes().map(|b| b.saturating_sub(b'0').min(5)).collect();
        p.ended = match text("endResult").as_str() {
            "1-0" => Some("1-0"),
            "0-1" => Some("0-1"),
            "1/2-1/2" => Some("1/2-1/2"),
            _ => None,
        }
        .map(|result| Ended { text: text("endText"), result });
        // The clocks go back to what they read; time with the page closed does not count.
        let saved = get("clock");
        if p.clock.is_some() && js_sys::Array::is_array(&saved) {
            let a = js_sys::Array::from(&saved);
            let left = [a.get(0).as_f64().unwrap_or(0.0), a.get(1).as_f64().unwrap_or(0.0)];
            let plies = p.g.history().len();
            let running = plies > 0 && !p.status().over;
            if let Some(c) = &mut p.clock {
                c.left = left;
                c.log = vec![left; plies];
                c.since = running.then(now);
            }
        } else {
            p.clock = None;
        }
        if !p.g.history().is_empty() {
            verdict("play-verdict", "", "Coach", "Welcome back. Your game is as you left it.");
        }
        self.play_refresh();
        self.play_prime();
        self.play_cpu_move();
        true
    }

    // ---- PGN and FEN ----

    fn play_pgn(&mut self) -> String {
        let p = &mut self.play;
        let level = format!("Computer, level {}", p.level + 1);
        let (white, black) = match (p.online.as_ref().map(|o| o.me), p.cpu, p.me) {
            (Some(0), ..) => ("Player", "Friend"),
            (Some(1), ..) => ("Friend", "Player"),
            (None, true, 0) => ("Player", level.as_str()),
            (None, true, _) => (level.as_str(), "Player"),
            _ => ("White", "Black"),
        };
        let d = js_sys::Date::new_0();
        let date = format!("{:04}.{:02}.{:02}", d.get_full_year(), d.get_month() + 1, d.get_date());
        let tags = [("Event", "Sixty-Four Chess"), ("Site", "?"), ("Date", date.as_str()), ("Round", "-"), ("White", white), ("Black", black)];
        let moves: Vec<String> = p.g.history().iter().map(|h| h.san.clone()).collect();
        let result = p.result();
        let start = (p.start_fen != START).then_some(p.start_fen.as_str());
        engine::pgn::format(&tags, start, &moves, result)
    }

    fn play_download_pgn(&mut self) {
        let pgn = self.play_pgn();
        let d = js_sys::Date::new_0();
        let name = format!("sixty-four-{:04}-{:02}-{:02}.pgn", d.get_full_year(), d.get_month() + 1, d.get_date());
        let Ok(a) = doc().create_element("a") else { return };
        let href = format!("data:application/x-chess-pgn;charset=utf-8,{}", String::from(js_sys::encode_uri_component(&pgn)));
        let _ = a.set_attribute("href", &href);
        let _ = a.set_attribute("download", &name);
        if let Some(body) = doc().body() {
            let _ = body.append_child(&a);
            a.unchecked_ref::<HtmlElement>().click();
            a.remove();
        }
        set_text("io-msg", &format!("Saved as {name}."));
    }

    /// Read what was pasted into the box and, if it makes sense, load it.
    fn play_load_text(&mut self) {
        let text = el("io-text").unchecked_into::<HtmlTextAreaElement>().value();
        let read = read_game_text(&text, &mut self.play.past);
        if self.play.view.is_some() {
            // Checking the text used the scratch board; put the position being looked at back on it.
            self.play_refresh();
        }
        match read {
            Ok(loaded) => {
                set_text("io-msg", "");
                self.play_request(Confirm::Load(loaded));
            }
            Err(why) => set_text("io-msg", &why),
        }
    }

    fn play_load(&mut self, loaded: Loaded) {
        self.sound.play(Sfx::Setup, 0.0);
        match loaded {
            Loaded::Position(fen) => {
                if self.play.cpu {
                    self.play.me = if fen.split(' ').nth(1) == Some("b") { 1 } else { 0 };
                }
                self.play_sync_controls();
                self.play_new_game(Some(&fen));
                set_text("io-msg", "Position loaded.");
            }
            Loaded::Game { start, moves, result } => {
                // A loaded game belongs to whoever played it, so the computer stays out of it.
                self.play.cpu = false;
                self.play_sync_controls();
                self.play_new_game(Some(&start));
                // Nobody is on the clock in a game that has already been played.
                self.play.clock = None;
                let p = &mut self.play;
                for san in &moves {
                    p.g.play_san(san);
                }
                if !p.g.status().over {
                    p.ended = match result.as_deref() {
                        Some("1-0") => Some(Ended { text: "White won.".into(), result: "1-0" }),
                        Some("0-1") => Some(Ended { text: "Black won.".into(), result: "0-1" }),
                        Some("1/2-1/2") => Some(Ended { text: "The game was drawn.".into(), result: "1/2-1/2" }),
                        _ => None,
                    };
                }
                // A finished game is for going through, so begin at the start of it.
                let finished = p.status().over;
                p.view = finished.then_some(0);
                let n = p.g.history().len().div_ceil(2);
                verdict("play-verdict", "", "Coach", "Game loaded. Use the arrows under the move list, or click a move, to go through it.");
                self.play_refresh();
                self.play_prime();
                set_text("io-msg", &format!("Game loaded: {n} move{}.", if n == 1 { "" } else { "s" }));
            }
        }
    }

    // ================================================================ LEARN

    fn learn_list(&self) {
        let mut h = String::new();
        let mut group = "";
        for (i, l) in LESSONS.iter().enumerate() {
            if l.group != group {
                if !group.is_empty() {
                    h.push_str("</div>");
                }
                group = l.group;
                h.push_str(&format!("<div class=\"grp\"><h3>{group}</h3>"));
            }
            h.push_str(&format!(
                "<button class=\"item{}\" data-i=\"{i}\"><span class=\"tick{}\"></span>{}</button>",
                if i == self.learn.cur { " on" } else { "" },
                if self.progress.lessons.contains(l.id) { " done" } else { "" },
                l.title
            ));
        }
        h.push_str("</div>");
        set_html("learn-list", &h);
        let n = LESSONS.iter().filter(|l| self.progress.lessons.contains(l.id)).count();
        set_style(&el("learn-meter"), "width", &format!("{}%", 100.0 * n as f64 / LESSONS.len() as f64));
        set_text("learn-count", &format!("{n} of {} lessons completed", LESSONS.len()));
    }

    fn learn_draw(&mut self, last: Option<(usize, usize)>) {
        let l = &LESSONS[self.learn.cur];
        let s = &mut self.learn;
        let mut marks: Vec<(usize, &'static str)> = Vec::new();
        if !s.solved {
            marks.extend(l.marks.iter().map(|m| (sq_of(m), "ring")));
            if let Some(show) = l.show {
                let from = sq_of(show);
                marks.extend(s.g.legal().iter().filter(|m| m.from as usize == from).map(|m| (m.to as usize, "ring")));
            }
        } else if let Some(target) = l.task.as_ref().and_then(|t| t.square) {
            marks.push((sq_of(target), "star"));
        }
        let b = &mut self.boards[LEARN];
        b.sync(&s.g);
        b.marks = marks;
        b.last = last;
        b.render();
    }

    fn learn_done(&mut self) {
        self.progress.lessons.insert(LESSONS[self.learn.cur].id.to_string());
        self.progress.save();
        self.learn_list();
    }

    fn learn_open(&mut self, i: usize) {
        let l = &LESSONS[i];
        self.learn.cur = i;
        self.learn.solved = false;
        self.learn.g.load(l.fen.unwrap_or(START));
        self.boards[LEARN].clear();
        set_text("learn-title", l.title);
        set_html("learn-body", l.body);
        set_text("learn-next", if i == LESSONS.len() - 1 { "Go to puzzles" } else { "Next lesson" });
        match &l.task {
            Some(t) => {
                verdict("learn-task", "", "Your turn", t.prompt);
                self.learn_list();
            }
            None => {
                verdict("learn-task", "ok", "Read", "Nothing to solve here. This lesson is marked complete.");
                self.learn_done();
            }
        }
        self.learn_draw(None);
    }

    fn learn_on_move(&mut self, m: Move) {
        let Some(task) = &LESSONS[self.learn.cur].task else { return };
        let s = strip(&self.learn.g.san(m, None));
        if task.accept.contains(&s.as_str()) {
            self.learn.g.play(m);
            self.learn.solved = true;
            let sfx = sfx_for(&mut self.learn.g, m);
            self.show_move(LEARN, m, sfx);
            verdict("learn-task", "ok", "Correct", task.ok);
            self.learn_done();
            self.learn_draw(Some((m.from as usize, m.to as usize)));
        } else {
            let text = format!("{} is legal, but it is not the move this lesson is after. {}", san_tag(&s), task.prompt);
            verdict("learn-task", "no", "Not quite", &text);
            self.boards[LEARN].dropped = false;
            self.sound.play(Sfx::Reject, 0.0);
            self.learn_draw(None);
        }
    }

    fn learn_on_square(&mut self, sq: usize) {
        let Some(task) = &LESSONS[self.learn.cur].task else { return };
        let Some(target) = task.square else { return };
        if self.learn.solved {
            return;
        }
        if name(sq) == target {
            self.learn.solved = true;
            self.sound.play(Sfx::Select, 0.0);
            verdict("learn-task", "ok", "Correct", task.ok);
            self.learn_done();
            self.learn_draw(None);
        } else {
            self.sound.play(Sfx::Select, 0.0);
            verdict("learn-task", "no", "Not quite", &format!("That square is {}. {}", name(sq), task.prompt));
        }
    }

    fn learn_next(&mut self) {
        if self.learn.cur < LESSONS.len() - 1 {
            self.learn_open(self.learn.cur + 1);
        } else {
            let _ = window().location().set_hash("#puzzles");
        }
    }

    // ================================================================ OPENINGS

    fn open_fen(&self) -> String {
        match self.open.ply {
            0 => START.to_string(),
            p => self.open.line[p - 1].fen.clone(),
        }
    }

    fn open_draw(&mut self) {
        let o = &OPENINGS[self.open.cur];
        let fen = self.open_fen();
        let s = &mut self.open;
        s.g.load(&fen);
        let b = &mut self.boards[OPEN];
        b.sync(&s.g);
        b.last = s.ply.checked_sub(1).map(|p| (s.line[p].m.from as usize, s.line[p].m.to as usize));
        b.render();

        let cell = |p: usize| {
            format!(
                "<span class=\"ply{}\" data-p=\"{p}\">{}</span>",
                if s.ply == p { " cur" } else { "" },
                s.line.get(p - 1).map(|x| x.san.as_str()).unwrap_or("")
            )
        };
        let mut h = String::new();
        for i in (0..s.line.len()).step_by(2) {
            h.push_str(&format!("<span class=\"n\">{}.</span>{}{}", i / 2 + 1, cell(i + 1), cell(i + 2)));
        }
        set_html("open-moves", &h);

        let note = o.notes.iter().find(|(p, _)| *p == s.ply).map(|(_, n)| *n);
        if s.ply == 0 {
            verdict("open-note", "", "Start", note.unwrap_or("Press Next to step through the moves."));
        } else {
            let label = format!(
                "{}{} {}",
                (s.ply + 1) / 2,
                if s.ply % 2 == 1 { "." } else { "…" },
                s.line[s.ply - 1].san
            );
            let fallback = if s.ply == s.line.len() {
                "End of the main line. From here, play on with the ideas below."
            } else {
                "Step on to see the next move."
            };
            verdict("open-note", "ok", &label, note.unwrap_or(fallback));
        }
        for id in ["open-prev", "open-first"] {
            set_disabled(id, s.ply == 0);
        }
        for id in ["open-next", "open-last"] {
            set_disabled(id, s.ply >= s.line.len());
        }
    }

    fn open_open(&mut self, i: usize) {
        let o = &OPENINGS[i];
        let s = &mut self.open;
        s.cur = i;
        s.ply = 0;
        self.boards[OPEN].clear();
        s.line.clear();
        s.g.load(START);
        for san in o.moves.split(' ') {
            let Some(m) = s.g.play_san(san) else { break };
            let san = s.g.history().last().map(|h| h.san.clone()).unwrap_or_default();
            s.line.push(Ply { m, san, fen: s.g.fen() });
        }
        let items: String = OPENINGS
            .iter()
            .enumerate()
            .map(|(k, x)| format!("<button class=\"item{}\" data-i=\"{k}\">{}</button>", if k == i { " on" } else { "" }, x.name))
            .collect();
        set_html("open-list", &format!("<div class=\"grp\"><h3>Openings</h3>{items}</div>"));
        set_html(
            "open-title",
            &format!("{} <span class=\"muted small\" style=\"font-family:var(--mono);font-weight:400\">{}</span>", o.name, o.eco),
        );
        let list = |xs: &[&str]| xs.iter().map(|x| format!("<li>{x}</li>")).collect::<String>();
        set_html(
            "open-body",
            &format!(
                "<p>{}</p><p><b>White aims to</b></p><ul>{}</ul><p><b>Black aims to</b></p><ul>{}</ul>",
                o.about,
                list(o.white),
                list(o.black)
            ),
        );
        self.open_draw();
    }

    fn open_go(&mut self, ply: isize) {
        let old = self.open.ply;
        let new = ply.clamp(0, self.open.line.len() as isize) as usize;
        self.open.ply = new;
        // Single steps slide the piece; jumps just show the position.
        let step = if new == old + 1 {
            Some((self.open.line[new - 1].m, true))
        } else if new + 1 == old {
            Some((self.open.line[old - 1].m, false))
        } else {
            None
        };
        let pairs = match step {
            Some((m, true)) => move_pairs(m),
            Some((m, false)) => back_pairs(m),
            None => Vec::new(),
        };
        self.boards[OPEN].slide(pairs);
        self.open_draw();
        if let Some((m, forward)) = step {
            let sfx = if forward { sfx_for(&mut self.open.g, m) } else { (Sfx::Move, 0) };
            let at = self.sound_at(OPEN, m, sfx.1);
            self.sound.play_at(sfx.0, move_ms(), at);
        }
    }

    fn open_step(&mut self, by: isize) {
        self.open_go(self.open.ply as isize + by);
    }

    // ================================================================ PUZZLES

    fn pz_label(i: usize) -> String {
        let p = &PUZZLES[i];
        if p.mate > 0 {
            format!("Mate in {}", p.mate)
        } else {
            p.theme.to_string()
        }
    }

    fn pz_grid(&self) {
        let h: String = PUZZLES
            .iter()
            .enumerate()
            .map(|(i, p)| {
                format!(
                    "<button class=\"pz{}{}\" data-i=\"{i}\"><b>{}</b>{}</button>",
                    if i == self.pz.cur { " on" } else { "" },
                    if self.progress.puzzles.contains(p.id) { " done" } else { "" },
                    i + 1,
                    Self::pz_label(i)
                )
            })
            .collect();
        set_html("pz-grid", &h);
        let n = PUZZLES.iter().filter(|p| self.progress.puzzles.contains(p.id)).count();
        set_text("pz-count", &format!("{n} / {} solved", PUZZLES.len()));
    }

    fn pz_draw(&mut self) {
        let s = &self.pz;
        let b = &mut self.boards[PUZZLE];
        b.sync(&s.g);
        b.flipped = PUZZLES[s.cur].fen.split(' ').nth(1) == Some("b");
        b.last = s.g.history().last().map(|h| (h.m.from as usize, h.m.to as usize));
        b.marks = s.hint.iter().map(|&sq| (sq, "hint")).collect();
        b.render();
    }

    fn pz_open(&mut self, i: usize) {
        let p = &PUZZLES[i];
        let s = &mut self.pz;
        s.timer += 1;
        s.cur = i;
        s.g.load(p.fen);
        s.left = p.mate;
        s.idx = 0;
        s.state = PzState::Play;
        s.hint = None;
        let side = if s.g.turn() == 1 { "Black" } else { "White" };
        self.boards[PUZZLE].clear();
        set_text("pz-title", &format!("Puzzle {} · {}", i + 1, Self::pz_label(i)));
        let prompt = if p.mate > 0 {
            format!("{side} to move and checkmate in {} move{}.", p.mate, if p.mate > 1 { "s" } else { "" })
        } else {
            format!("{side} to move and win material. Theme: {}.", p.theme.to_lowercase())
        };
        set_text("pz-prompt", &prompt);
        verdict("pz-feedback", "", "Your move", "Drag a piece or tap it and then its target square.");
        self.pz_grid();
        self.pz_draw();
    }

    fn pz_win(&mut self) {
        self.pz.state = PzState::Done;
        self.progress.puzzles.insert(PUZZLES[self.pz.cur].id.to_string());
        self.progress.save();
        self.pz_grid();
        let text = if self.pz.cur < PUZZLES.len() - 1 {
            "Well done. On to the next one."
        } else {
            "Well done. That was the last puzzle."
        };
        verdict("pz-feedback", "ok", "Solved", text);
    }

    /// Let the correct move sit on the board for a moment, then play the defender's reply.
    fn pz_schedule_reply(&mut self) {
        self.pz.state = PzState::Wait;
        self.pz.timer += 1;
        let timer = self.pz.timer;
        set_timeout(min_reply_ms() as i32, move || with_app(|a| a.pz_reply(timer)));
    }

    fn pz_reply(&mut self, timer: u32) {
        if timer != self.pz.timer {
            return;
        }
        let p = &PUZZLES[self.pz.cur];
        let s = &mut self.pz;
        let reply = if p.mate > 0 {
            // Any defence that is not immediately mate; all of them lose in time.
            let ms = s.g.legal();
            let pick = ms.iter().copied().find(|&x| {
                s.g.play(x);
                let alive = !s.g.status().mate;
                s.g.takeback();
                alive
            });
            let reply = pick.or(ms.first().copied());
            if let Some(m) = reply {
                s.g.play(m);
            }
            reply
        } else if let Some(san) = p.line.get(s.idx) {
            s.idx += 1;
            s.g.play_san(san)
        } else {
            None
        };
        s.state = PzState::Play;
        if let Some(m) = reply {
            let sfx = sfx_for(&mut self.pz.g, m);
            self.show_move(PUZZLE, m, sfx);
        }
        self.pz_draw();
    }

    fn pz_on_move(&mut self, m: Move) {
        let p = &PUZZLES[self.pz.cur];
        let s = &mut self.pz;
        s.hint = None;
        let san = s.g.play(m);
        let sfx = sfx_for(&mut s.g, m);
        let dropped = self.boards[PUZZLE].dropped;
        let landed = self.boards[PUZZLE].slide(move_pairs(m));
        let at = self.sound_at(PUZZLE, m, sfx.1);
        let s = &mut self.pz;
        if p.mate > 0 {
            if s.g.status().mate {
                self.sound.play_at(sfx.0, landed, at);
                self.pz_draw();
                self.pz_win();
                return;
            }
            if s.left > 1 && s.g.mated_within(s.left - 1) {
                s.left -= 1;
                self.sound.play_at(sfx.0, landed, at);
                self.pz_draw();
                verdict("pz-feedback", "ok", "Correct", &format!("{} keeps the mate. Finish it.", san_tag(&san)));
                self.pz_schedule_reply();
                return;
            }
            let left = s.left;
            s.g.takeback();
            self.pz_reject(m, dropped);
            verdict("pz-feedback", "no", "Try again", &format!("{} does not force mate in {left}.", san_tag(&san)));
        } else {
            if p.line.get(s.idx) == Some(&strip(&san).as_str()) {
                s.idx += 1;
                let finished = s.idx >= p.line.len();
                self.sound.play_at(sfx.0, landed, at);
                self.pz_draw();
                if finished {
                    self.pz_win();
                    return;
                }
                verdict("pz-feedback", "ok", "Correct", &format!("{} is right. Now collect.", san_tag(&san)));
                self.pz_schedule_reply();
                return;
            }
            s.g.takeback();
            self.pz_reject(m, dropped);
            verdict("pz-feedback", "no", "Try again", &format!("{} is not the strongest move here.", san_tag(&san)));
        }
    }

    /// A wrong try has been taken back: a dropped piece slides home, a clicked one never left.
    fn pz_reject(&mut self, m: Move, dropped: bool) {
        self.boards[PUZZLE].slide(if dropped { back_pairs(m) } else { Vec::new() });
        self.sound.play(Sfx::Reject, 0.0);
        self.pz_draw();
    }

    fn pz_hint(&mut self) {
        let s = &mut self.pz;
        if s.state != PzState::Play {
            return;
        }
        let p = &PUZZLES[s.cur];
        let m = if p.mate > 0 {
            s.g.find_mate(s.left)
        } else {
            let ms = s.g.legal();
            let want = p.line.get(s.idx).copied().unwrap_or("");
            ms.iter().copied().find(|&x| strip(&s.g.san(x, Some(&ms))) == want)
        };
        if let Some(m) = m {
            s.hint = Some(m.from as usize);
            self.pz_draw();
            verdict("pz-feedback", "", "Hint", "Look at the marked piece.");
        }
    }

    // ================================================================ routing

    fn route(&mut self) {
        let hash = window().location().hash().unwrap_or_default();
        self.view = VIEWS.iter().position(|v| hash.strip_prefix('#') == Some(*v)).unwrap_or(PLAY);
        let current = VIEWS[self.view];
        for v in VIEWS {
            let _ = el(&format!("view-{v}")).class_list().toggle_with_force("on", v == current);
        }
        if let Ok(links) = doc().query_selector_all("#nav a") {
            for i in 0..links.length() {
                if let Some(a) = links.item(i).and_then(|n| n.dyn_into::<Element>().ok()) {
                    let on = a.get_attribute("href").as_deref() == Some(&format!("#{current}"));
                    let _ = a.class_list().toggle_with_force("on", on);
                }
            }
        }
    }
}

/// Show the coach's verdict on a move and return its class: 1 best … 5 blunder.
fn show_judgement(j: &Judge, best: &SearchOut, after: Option<&SearchOut>) -> u8 {
    let clamp = |x: i32| x.clamp(-1500, 1500);
    // Both scores from the mover's point of view; `after` is scored for the opponent.
    let played = after.map(|b| -b.score).unwrap_or(0);
    let loss = clamp(best.score) - clamp(played);
    let same = best.from == j.m.from && best.to == j.m.to && best.promo == j.m.promo;
    let alt = san_tag(&best.san);
    let (class, cls, label, text) = if same || loss <= 15 {
        (1, "best", "Best", String::from("The engine agrees with this move."))
    } else if loss <= 60 {
        (2, "good", "Good", String::from("A sound move."))
    } else if loss <= 150 {
        (3, "inacc", "Inaccuracy", format!("There was something a little better: {alt}."))
    } else if loss <= 350 {
        (4, "mistake", "Mistake", format!("This gives up about {:.1} pawns. Stronger was {alt}.", loss as f64 / 100.0))
    } else {
        (5, "blunder", "Blunder", format!("This loses material or worse. {alt} was the move. Undo to try it."))
    };
    verdict("play-verdict", cls, label, &format!("{} {text}", san_tag(&j.san)));
    class
}

/// The engine runs in a module worker so searching never freezes the page.
fn start_worker() -> Option<Worker> {
    let opts = WorkerOptions::new();
    opts.set_type(WorkerType::Module);
    let w = Worker::new_with_options("./worker.js", &opts).ok()?;
    let on_msg = Closure::<dyn FnMut(MessageEvent)>::new(|e: MessageEvent| {
        if let Some((id, out)) = e.data().as_string().and_then(|s| decode_reply(&s)) {
            with_app(|a| a.on_result(id, out));
        }
    });
    w.set_onmessage(Some(on_msg.as_ref().unchecked_ref()));
    on_msg.forget();
    let on_err = Closure::<dyn FnMut(Event)>::new(|e: Event| {
        e.prevent_default();
        with_app(|a| a.worker_failed());
    });
    w.set_onerror(Some(on_err.as_ref().unchecked_ref()));
    on_err.forget();
    Some(w)
}

fn show_sound_state(on: bool) {
    let b = el("btn-sound");
    b.set_text_content(Some(if on { "Sound on" } else { "Sound off" }));
    let _ = b.set_attribute("aria-pressed", if on { "true" } else { "false" });
}

fn select_value(e: &Event) -> String {
    e.target().and_then(|t| t.dyn_into::<HtmlSelectElement>().ok()).map(|s| s.value()).unwrap_or_default()
}

fn wire_events() {
    let win: EventTarget = window().into();
    for (w, id) in ["play-board", "learn-board", "open-board", "pz-board"].into_iter().enumerate() {
        let board: EventTarget = el(id).into();
        listen::<PointerEvent>(&board, "pointerdown", move |a, e| a.board_down(w, e));
        listen::<KeyboardEvent>(&board, "keydown", move |a, e| a.board_key(w, e));
        // The right button draws arrows, so it must not also open the browser's menu.
        listen::<Event>(&board, "contextmenu", |_, e| e.prevent_default());
        // A promotion chosen with the keyboard arrives as a click on its button.
        listen::<Event>(&board, "click", move |a, e| {
            if let Some(piece) = data_index(&e, "[data-promo]", "data-promo") {
                a.board_promo(w, Some(piece as u8));
            }
        });
        // Reaching the board with Tab (rather than by clicking it) starts keyboard play.
        listen::<FocusEvent>(&board, "focus", move |a, _| {
            let b = &mut a.boards[w];
            if !b.kbd && b.root.matches(":focus-visible").unwrap_or(false) {
                b.kbd = true;
                b.render();
            }
        });
        // Focus going to another control ends keyboard play on this board.
        listen::<FocusEvent>(&board, "focusout", move |a, e| {
            let b = &mut a.boards[w];
            let next = e.related_target().and_then(|t| t.dyn_into::<web_sys::Node>().ok());
            if b.kbd && next.is_some_and(|n| !b.root.contains(Some(&n))) {
                b.kbd = false;
                b.render();
            }
        });
    }
    listen::<PointerEvent>(&win, "pointerdown", |a, e| {
        a.boards.iter_mut().for_each(|b| b.kbd = false);
        // A press anywhere outside the settings panel closes it.
        let target = e.target().and_then(|t| t.dyn_into::<Element>().ok());
        if !target.is_some_and(|t| t.closest("#settings, #btn-settings").ok().flatten().is_some()) {
            a.settings_open(false);
        }
    });
    listen::<PointerEvent>(&win, "pointermove", |a, e| a.boards.iter_mut().for_each(|b| b.drag_move(&e)));
    listen::<PointerEvent>(&win, "pointerup", |a, e| a.pointer_end(e, true));
    listen::<PointerEvent>(&win, "pointercancel", |a, e| a.pointer_end(e, false));
    listen::<Event>(&win, "hashchange", |a, _| a.route());

    on_click("btn-sound", |a| {
        a.sound.set_on(!a.sound.on);
        if let Ok(Some(s)) = window().local_storage() {
            let _ = s.set_item(SOUND_KEY, if a.sound.on { "1" } else { "0" });
        }
        a.sound.play(Sfx::Move, 0.0);
        show_sound_state(a.sound.on);
    });

    on_click("btn-settings", |a| a.settings_toggle());
    for id in ["set-board", "set-pieces", "set-speed"] {
        listen::<Event>(&el(id), "change", |a, _| a.settings_changed());
    }
    listen::<Event>(&el("set-volume"), "input", |a, _| a.settings_changed());
    // Letting go of the slider plays a move, so the new volume can be heard.
    listen::<Event>(&el("set-volume"), "change", |a, _| a.sound.play(Sfx::Move, 0.0));

    // Play
    set_interval(100, || with_app(|a| a.clock_tick()));
    on_click("btn-invite", |a| a.play_request(Confirm::Invite));
    on_click("btn-copy-link", |a| a.online_copy_link());
    on_click("btn-leave", |a| a.play_request(Confirm::NewGame));
    on_click("btn-san", |a| a.play_typed());
    listen::<KeyboardEvent>(&el("play-san"), "keydown", |a, e| {
        if e.key() == "Enter" {
            e.prevent_default();
            a.play_typed();
        }
    });
    listen::<Event>(&el("sel-time"), "change", |a, e| {
        a.play.tc = select_value(&e).parse().unwrap_or(0).min(TIME_CONTROLS.len() - 1);
        let p = &mut a.play;
        if p.g.history().is_empty() && p.ended.is_none() && p.online.is_none() {
            // Nothing has been played yet, so the choice applies to this game.
            a.clock_reset();
            set_text("time-note", "");
            a.play_refresh();
        } else {
            set_text("time-note", "The new clock starts with the next game.");
            a.play_save();
        }
    });
    on_click("btn-new", |a| a.play_request(Confirm::NewGame));
    on_click("btn-resign", |a| a.play_request(Confirm::Resign));
    on_click("btn-draw", |a| a.play_offer_draw());
    on_click("confirm-yes", |a| a.play_answer(true));
    on_click("confirm-no", |a| a.play_answer(false));
    on_click("play-first", |a| a.play_go(0));
    on_click("play-prev", |a| a.play_step(-1));
    on_click("play-next", |a| a.play_step(1));
    on_click("play-last", |a| a.play_go(isize::MAX));
    listen::<Event>(&el("play-moves"), "click", |a, e| {
        if let Some(p) = data_index(&e, ".ply", "data-p") {
            a.play_go(p as isize);
        }
    });
    on_click("btn-copy-pgn", |a| offer_text(&a.play_pgn(), "The PGN"));
    on_click("btn-copy-fen", |a| offer_text(&a.play_shown_fen(), "The FEN"));
    on_click("btn-save-pgn", |a| a.play_download_pgn());
    on_click("btn-load", |a| a.play_load_text());
    on_click("btn-undo", |a| a.play_undo());
    on_click("btn-hint", |a| a.play_hint());
    on_click("btn-flip", |a| {
        a.play.flipped = !a.play.flipped;
        a.play_refresh();
        a.play_prime();
    });
    listen::<Event>(&el("sel-mode"), "change", |a, e| {
        a.play.cpu = select_value(&e) == "cpu";
        a.play.token += 1;
        a.play.thinking = false;
        a.play_refresh();
        a.play_cpu_move();
    });
    listen::<Event>(&el("sel-level"), "change", |a, e| {
        a.play.level = select_value(&e).parse().unwrap_or(2).min(LEVELS.len() - 1);
        a.play_save();
    });
    listen::<Event>(&el("sel-color"), "change", |a, e| {
        let me = select_value(&e).parse().unwrap_or(0) & 1;
        a.play_request(Confirm::NewGameAs(me));
    });
    listen::<Event>(&el("chk-coach"), "change", |a, e| {
        a.play.coach = e.target().and_then(|t| t.dyn_into::<HtmlInputElement>().ok()).is_some_and(|c| c.checked());
        if !a.play.coach {
            a.play.judge = None;
            verdict("play-verdict", "", "Coach", "Coach is off.");
        }
        a.play_save();
    });

    // Learn
    listen::<Event>(&el("learn-list"), "click", |a, e| {
        if let Some(i) = data_index(&e, ".item", "data-i") {
            a.learn_open(i);
        }
    });
    on_click("learn-reset", |a| a.learn_open(a.learn.cur));
    on_click("learn-next", |a| a.learn_next());

    // Openings
    listen::<Event>(&el("open-list"), "click", |a, e| {
        if let Some(i) = data_index(&e, ".item", "data-i") {
            a.open_open(i);
        }
    });
    listen::<Event>(&el("open-moves"), "click", |a, e| {
        if let Some(p) = data_index(&e, ".ply", "data-p").filter(|p| *p <= a.open.line.len()) {
            a.open_go(p as isize);
        }
    });
    on_click("open-first", |a| a.open_go(0));
    on_click("open-prev", |a| a.open_step(-1));
    on_click("open-next", |a| a.open_step(1));
    on_click("open-last", |a| a.open_go(isize::MAX));
    on_click("open-play", |a| {
        let fen = a.open_fen();
        a.play_request(Confirm::StartFrom(fen));
        let _ = window().location().set_hash("#play");
    });
    listen::<KeyboardEvent>(&doc().into(), "keydown", |a, e| {
        let in_field = e
            .target()
            .and_then(|t| t.dyn_into::<Element>().ok())
            .is_some_and(|t| matches!(t.tag_name().as_str(), "SELECT" | "INPUT" | "TEXTAREA"));
        if e.key() == "Escape" {
            a.settings_open(false);
        }
        if in_field || e.ctrl_key() || e.meta_key() || e.alt_key() {
            return;
        }
        // Arrow keys step through the opening line, or back and forth through the game.
        match (a.view, e.key().as_str()) {
            (OPEN, "ArrowRight") => a.open_step(1),
            (OPEN, "ArrowLeft") => a.open_step(-1),
            (OPEN, "Home") => a.open_go(0),
            (OPEN, "End") => a.open_go(isize::MAX),
            (PLAY, "ArrowRight") => a.play_step(1),
            (PLAY, "ArrowLeft") => a.play_step(-1),
            (PLAY, "Home") => a.play_go(0),
            (PLAY, "End") => a.play_go(isize::MAX),
            _ => return,
        }
        e.prevent_default();
    });

    // Puzzles
    on_click("pz-hint", |a| a.pz_hint());
    on_click("pz-retry", |a| a.pz_open(a.pz.cur));
    on_click("pz-next", |a| a.pz_open((a.pz.cur + 1) % PUZZLES.len()));
    listen::<Event>(&el("pz-grid"), "click", |a, e| {
        if let Some(i) = data_index(&e, ".pz", "data-i") {
            a.pz_open(i);
        }
    });
}

/// The game named in an invitation link (`?g=<id>`). The id is then taken out of the
/// address bar, so that reloading the page later does not try to join all over again.
fn invitation() -> Option<String> {
    let loc = window().location();
    let query = loc.search().ok()?;
    let id = query.trim_start_matches('?').split('&').find_map(|p| p.strip_prefix("g="))?.to_string();
    if let Ok(history) = window().history() {
        let clean = format!("{}{}", loc.pathname().unwrap_or_default(), loc.hash().unwrap_or_default());
        let _ = history.replace_state_with_url(&JsValue::NULL, "", Some(&clean));
    }
    (!id.is_empty() && id.len() <= 16 && id.chars().all(|c| c.is_ascii_alphanumeric())).then_some(id)
}

#[wasm_bindgen(start)]
pub fn start() {
    // The same module is loaded inside the search worker, where there is no page to drive.
    if web_sys::window().is_none() {
        return;
    }
    let mut app = App::new();
    app.settings.apply();
    app.settings.show();
    app.sound.set_volume(app.settings.volume);
    let invited = invitation();
    if !app.play_restore() {
        app.play_new_game(None);
    }
    app.online_show();
    // Opened from a friend's link: take the other seat, unless this browser is already in that game.
    if let Some(id) = invited {
        if app.play.online.as_ref().map(|o| o.id.as_str()) != Some(id.as_str()) {
            app.online_join(id);
        }
    }
    let lesson = LESSONS.iter().position(|l| !app.progress.lessons.contains(l.id)).unwrap_or(0);
    app.learn_open(lesson);
    app.open_open(0);
    let puzzle = PUZZLES.iter().position(|p| !app.progress.puzzles.contains(p.id)).unwrap_or(0);
    app.pz_open(puzzle);
    app.route();
    show_sound_state(app.sound.on);
    APP.with(|a| *a.borrow_mut() = Some(app));
    wire_events();
}
