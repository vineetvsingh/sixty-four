//! Sound. Every effect is a recording of real chess pieces on a real wooden board
//! (see static/sounds and the credits in README.md), played through Web Audio.
//! Nothing is synthesised: a capture is a knock of wood on wood followed by the piece
//! being set down, check is the piece put down hard, and mate ends with a king falling over.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{AudioBuffer, AudioContext, GainNode, OfflineAudioContext, Response};

/// Pieces set down on the board; a different take is used each time.
const MOVES: [&str; 5] = ["move1", "move2", "move3", "move4", "move5"];
/// One piece knocking against another.
const CLACKS: [&str; 3] = ["clack1", "clack2", "clack3"];
/// A piece touched or lifted.
const TICKS: [&str; 2] = ["tick1", "tick2"];
const OTHERS: [&str; 4] = ["thump", "topple", "slide", "setup"];

#[derive(Clone, Copy, PartialEq)]
pub enum Sfx {
    Move,
    Capture,
    Castle,
    Promote,
    /// A piece picked up, or a square tapped.
    Select,
    /// A wrong try: the piece is slid back.
    Reject,
    /// The pieces being set up for a new game.
    Setup,
    /// A king laid on its side: resignation.
    Topple,
}

/// What made the sound and where: heavier pieces sound lower, and the sound sits
/// slightly left or right according to the file the piece lands on.
#[derive(Clone, Copy)]
pub struct At {
    /// Piece kind, 1 (pawn) to 6 (king); 0 if not a piece.
    pub piece: u8,
    /// File as shown on screen, 0.0 (left edge) to 7.0 (right edge).
    pub file: f32,
    /// How hard the piece comes down: 0 normally, 1 giving check, 2 giving checkmate.
    pub force: u8,
}

impl At {
    pub const CENTRE: At = At { piece: 0, file: 3.5, force: 0 };
}

type Samples = Rc<RefCell<HashMap<&'static str, AudioBuffer>>>;

pub struct Sound {
    pub on: bool,
    ctx: Option<AudioContext>,
    master: Option<GainNode>,
    samples: Samples,
    loading: bool,
    last_move: usize,
    /// 0.0 to 1.0, from the settings.
    volume: f64,
}

fn rnd(lo: f64, hi: f64) -> f64 {
    lo + js_sys::Math::random() * (hi - lo)
}

fn pick(n: usize) -> usize {
    ((js_sys::Math::random() * n as f64) as usize).min(n - 1)
}

async fn fetch_sample(name: &str) -> Result<AudioBuffer, JsValue> {
    let win = web_sys::window().ok_or(JsValue::NULL)?;
    let resp: Response = JsFuture::from(win.fetch_with_str(&format!("sounds/{name}.wav"))).await?.dyn_into()?;
    if !resp.ok() {
        return Err(JsValue::NULL);
    }
    let bytes: js_sys::ArrayBuffer = JsFuture::from(resp.array_buffer()?).await?.dyn_into()?;
    // An offline context may decode before the visitor has interacted with the page,
    // so the samples are ready by the time the first move is made.
    let decoder = OfflineAudioContext::new_with_number_of_channels_and_length_and_sample_rate(1, 1, 44100.0)?;
    JsFuture::from(decoder.decode_audio_data(&bytes)?).await?.dyn_into()
}

impl Sound {
    pub fn new(on: bool) -> Sound {
        let mut s =
            Sound { on, ctx: None, master: None, samples: Rc::new(RefCell::new(HashMap::new())), loading: false, last_move: 0, volume: 1.0 };
        if on {
            s.load();
        }
        s
    }

    pub fn set_on(&mut self, on: bool) {
        self.on = on;
        if on {
            self.load();
        }
    }

    pub fn set_volume(&mut self, volume: f64) {
        self.volume = volume.clamp(0.0, 1.0);
        if let Some(master) = &self.master {
            master.gain().set_value((0.9 * self.volume) as f32);
        }
    }

    fn load(&mut self) {
        if std::mem::replace(&mut self.loading, true) {
            return;
        }
        for name in MOVES.into_iter().chain(CLACKS).chain(TICKS).chain(OTHERS) {
            let samples = self.samples.clone();
            spawn_local(async move {
                if let Ok(buf) = fetch_sample(name).await {
                    samples.borrow_mut().insert(name, buf);
                }
            });
        }
    }

    /// Play `sfx` after `delay_ms`, typically the time a piece takes to land.
    pub fn play(&mut self, sfx: Sfx, delay_ms: f64) {
        self.play_at(sfx, delay_ms, At::CENTRE);
    }

    pub fn play_at(&mut self, sfx: Sfx, delay_ms: f64, at: At) {
        if self.on {
            let _ = self.try_play(sfx, delay_ms, at);
        }
    }

    /// A take of a piece being set down that differs from the one just heard.
    fn next_move(&mut self) -> &'static str {
        let mut i = pick(MOVES.len());
        if i == self.last_move {
            i = (i + 1 + pick(MOVES.len() - 1)) % MOVES.len();
        }
        self.last_move = i;
        MOVES[i]
    }

    fn try_play(&mut self, sfx: Sfx, delay_ms: f64, at: At) -> Result<(), JsValue> {
        // Browsers only allow audio after a user gesture, so the context is created on first use.
        if self.ctx.is_none() {
            let ctx = AudioContext::new()?;
            let master = ctx.create_gain()?;
            master.gain().set_value((0.9 * self.volume) as f32);
            master.connect_with_audio_node(&ctx.destination())?;
            self.master = Some(master);
            self.ctx = Some(ctx);
        }
        let (Some(ctx), Some(master)) = (self.ctx.clone(), self.master.clone()) else { return Ok(()) };
        let _ = ctx.resume();
        let t = ctx.current_time() + delay_ms / 1000.0 + 0.005;
        let pan = ((at.file - 3.5) / 3.5 * 0.3).clamp(-1.0, 1.0);
        // Playback rate by weight: a pawn is a light, bright knock; a king is a lower one.
        let weight = |piece: u8| match piece {
            1 => 1.06,
            4 => 0.96,
            5 => 0.93,
            6 => 0.90,
            _ => 1.0,
        };
        let w = weight(at.piece) * if at.force > 0 { 0.97 } else { 1.0 };
        let samples = self.samples.clone();
        let hit = |name: &str, when: f64, gain: f64, rate: f64| -> Result<(), JsValue> {
            let Some(buf) = samples.borrow().get(name).cloned() else { return Ok(()) };
            let src = ctx.create_buffer_source()?;
            src.set_buffer(Some(&buf));
            // No two knocks on a real board are identical: vary pitch and strength a little.
            src.playback_rate().set_value((rate * rnd(0.97, 1.03)) as f32);
            let g = ctx.create_gain()?;
            g.gain().set_value((gain * rnd(0.86, 1.0)) as f32);
            let p = ctx.create_stereo_panner()?;
            p.pan().set_value(pan);
            src.connect_with_audio_node(&g)?.connect_with_audio_node(&p)?.connect_with_audio_node(&master)?;
            src.start_with_when(when)
        };
        let first = self.next_move();
        // When the final placement of the move happens, for the emphasis below.
        let mut landed = t;
        match sfx {
            Sfx::Move => hit(first, t, 0.85, w)?,
            Sfx::Capture => {
                // The capturing piece knocks the other one, then is set down in its place.
                landed = t + 0.065;
                hit(CLACKS[pick(CLACKS.len())], t, 0.5, 1.0)?;
                hit(first, landed, 0.95, w * 0.96)?;
            }
            Sfx::Castle => {
                // King first, then the rook hops over.
                hit(first, t, 0.85, weight(6))?;
                hit(self.next_move(), t + 0.24, 0.8, weight(4))?;
            }
            Sfx::Promote => {
                // The pawn arrives, then is swapped for the new piece.
                landed = t + 0.3;
                hit(first, t, 0.8, weight(1))?;
                hit(self.next_move(), landed, 0.9, weight(5))?;
            }
            Sfx::Select => hit(TICKS[pick(TICKS.len())], t, 0.4, w)?,
            Sfx::Reject => hit("slide", t, 0.6, 1.0)?,
            Sfx::Setup => hit("setup", t, 0.6, 1.0)?,
            Sfx::Topple => hit("topple", t, 0.75, 1.0)?,
        }
        // Check: the piece is put down hard enough for the table to sound.
        // Checkmate: harder still, and a moment later the king is tipped over.
        if at.force >= 1 {
            hit("thump", landed, if at.force >= 2 { 0.6 } else { 0.45 }, 0.97)?;
        }
        if at.force >= 2 {
            hit("topple", landed + 0.6, 0.75, 1.0)?;
        }
        Ok(())
    }
}
