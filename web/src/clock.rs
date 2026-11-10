//! Chess clocks for the Play board. Against the computer and with two players at
//! one screen the page keeps time itself; in an online game the server does, and
//! the page only counts down between its reports.

use super::*;

/// (minutes and seconds to start with, seconds added after each move), in seconds.
/// The first entry is "no clock". The labels are in index.html.
pub const TIME_CONTROLS: [(f64, f64); 7] =
    [(0.0, 0.0), (60.0, 0.0), (180.0, 2.0), (300.0, 0.0), (600.0, 0.0), (900.0, 10.0), (1800.0, 0.0)];

pub struct Clock {
    /// Milliseconds left for White and Black, as of `since` if the clock is running.
    pub left: [f64; 2],
    pub base: f64,
    pub inc: f64,
    /// When the side to move started thinking; `None` before the first move and after the game.
    pub since: Option<f64>,
    /// The clocks after each half-move, so that taking a move back gives the time back too.
    pub log: Vec<[f64; 2]>,
}

/// "5:00", "1:02:03", or with tenths in the last ten seconds: "0:07.4".
pub fn clock_text(ms: f64) -> String {
    let ms = ms.max(0.0);
    if ms < 10_000.0 {
        let tenths = (ms / 100.0).floor() as u64;
        return format!("0:{:02}.{}", tenths / 10, tenths % 10);
    }
    let s = (ms / 1000.0).ceil() as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Whether `side` still has the material to give checkmate at all.
fn can_mate(board: &[u8; 64], side: usize) -> bool {
    let mut minors = 0;
    for &p in board.iter().filter(|p| **p != 0 && (**p >> 3) as usize == side) {
        match p & 7 {
            engine::N | engine::B => minors += 1,
            engine::K => {}
            _ => return true,
        }
    }
    minors >= 2
}

impl App {
    /// Set the clocks for a new game from the chosen time control.
    pub fn clock_reset(&mut self) {
        let (base, inc) = TIME_CONTROLS[self.play.tc];
        self.play.clock = (base > 0.0).then(|| Clock {
            left: [base * 1000.0; 2],
            base: base * 1000.0,
            inc: inc * 1000.0,
            since: None,
            log: Vec::new(),
        });
    }

    /// Time left for `side` right now, if this game has a clock.
    pub fn clock_left(&self, side: usize) -> Option<f64> {
        let c = self.play.clock.as_ref()?;
        let running = c.since.filter(|_| self.play.g.turn() == side);
        Some((c.left[side] - running.map(|s| now() - s).unwrap_or(0.0)).max(0.0))
    }

    /// `mover` has just played: stop their clock, add the increment, start the other one.
    pub fn clock_moved(&mut self, mover: usize) {
        let over = self.play.status().over;
        let Some(c) = &mut self.play.clock else { return };
        let t = now();
        // The clocks start once the first move has been made.
        if let Some(since) = c.since {
            c.left[mover] = (c.left[mover] - (t - since)).max(0.0) + c.inc;
        }
        c.since = (!over).then_some(t);
        c.log.push(c.left);
    }

    /// Freeze the clocks where they stand: the game has ended.
    pub fn clock_stop(&mut self) {
        let side = self.play.g.turn();
        let left = self.clock_left(side);
        if let (Some(c), Some(left)) = (&mut self.play.clock, left) {
            if c.since.take().is_some() {
                c.left[side] = left;
            }
        }
    }

    /// Moves were taken back, or a finished game was put back on: set the clocks to match.
    pub fn clock_rewind(&mut self) {
        let plies = self.play.g.history().len();
        let over = self.play.status().over;
        let turn = self.play.g.turn();
        let Some(c) = &mut self.play.clock else { return };
        // The log holds the clocks as they stood after each move, so its last entry is what the
        // side to move had when its turn began. Going back there also undoes a fallen flag.
        if plies <= c.log.len() {
            c.log.truncate(plies);
            c.left = c.log.last().copied().unwrap_or([c.base; 2]);
        }
        if c.left[turn] <= 0.0 {
            c.left[turn] = c.inc.max(10_000.0);
        }
        c.since = (plies > 0 && !over).then(now);
    }

    fn clock_class(&self, side: usize, left: f64) -> String {
        let running = self.play.clock.as_ref().is_some_and(|c| c.since.is_some()) && self.play.g.turn() == side;
        format!("clock{}{}", if running { " run" } else { "" }, if left < 20_000.0 { " low" } else { "" })
    }

    /// The clock shown at the end of a player's name row, or nothing in an untimed game.
    pub fn clock_html(&self, side: usize) -> String {
        match self.clock_left(side) {
            Some(left) => {
                format!("<span class=\"{}\" data-side=\"{side}\">{}</span>", self.clock_class(side, left), clock_text(left))
            }
            None => String::new(),
        }
    }

    /// Runs ten times a second: counts the running clock down and calls a flag fall.
    pub fn clock_tick(&mut self) {
        if !self.play.clock.as_ref().is_some_and(|c| c.since.is_some()) {
            return;
        }
        for row in ["play-top", "play-bot"] {
            let Some(e) = el(row).query_selector(".clock").ok().flatten() else { continue };
            let side = usize::from(e.get_attribute("data-side").as_deref() == Some("1"));
            let left = self.clock_left(side).unwrap_or(0.0);
            e.set_class_name(&self.clock_class(side, left));
            e.set_text_content(Some(&clock_text(left)));
        }
        let side = self.play.g.turn();
        if self.clock_left(side).is_some_and(|left| left <= 0.0) {
            if self.play.online.is_some() {
                // The server keeps the official time; ask it to look.
                self.online_claim_flag();
            } else {
                self.clock_flag(side);
            }
        }
    }

    /// `side` ran out of time in a game the page is timing.
    fn clock_flag(&mut self, side: usize) {
        if let Some(c) = &mut self.play.clock {
            c.left[side] = 0.0;
            c.since = None;
        }
        let (text, result) = self.play_time_result(side, can_mate(self.play.g.board(), side ^ 1));
        self.sound.play(Sfx::Reject, 0.0);
        verdict("play-verdict", "", "Clock", "Time is up. Go back through the moves to see where the time went.");
        self.play_end(&text, result);
    }

    /// The words and the score for a game `loser` lost on time.
    pub fn play_time_result(&self, loser: usize, winner_can_mate: bool) -> (String, &'static str) {
        if winner_can_mate {
            (self.play_loss_text(loser, "ran out of time"), if loser == 0 { "0-1" } else { "1-0" })
        } else {
            let who = self.play_loss_text(loser, "ran out of time");
            let who = who.split('.').next().unwrap_or("");
            (format!("{who}, but there is not enough material left to checkmate. Draw."), "1/2-1/2")
        }
    }
}
