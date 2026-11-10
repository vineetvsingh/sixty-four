//! Games between two people in different browsers.
//!
//! One player creates a game and sends the other a link. The server keeps the
//! position, checks every move with the same engine the page uses, and runs the
//! clocks, so neither browser has to be trusted. Games live in memory only: they
//! are gone when the server restarts, and are dropped after six idle hours.

use engine::{Engine, START};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// New games are refused beyond this many, so the server cannot be filled up.
pub const MAX_GAMES: usize = 2000;
const IDLE: Duration = Duration::from_secs(6 * 3600);
const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";

/// (HTTP status, message shown to the player).
pub type Refusal = (u16, &'static str);

const NO_GAME: Refusal = (404, "That game no longer exists.");
const NOT_PLAYER: Refusal = (403, "You are not a player in this game.");

struct Clock {
    /// Milliseconds left for White and Black, as of `since` if the clock is running.
    left: [f64; 2],
    inc: f64,
    /// When the side to move started thinking. `None` before the first move and after the game.
    since: Option<Instant>,
}

struct Game {
    g: Engine,
    /// Secrets that prove who is White and who is Black.
    seats: [String; 2],
    creator: usize,
    joined: bool,
    /// Goes up on every change; clients ask for "anything newer than N".
    version: u64,
    /// "" while playing, else "board", "draw", "resign:<side>", "time:<side>" or "timedraw:<side>".
    end: String,
    draw_offer: Option<usize>,
    clock: Option<Clock>,
    touched: Instant,
}

impl Game {
    fn seat_of(&self, seat: &str) -> Option<usize> {
        if seat.is_empty() {
            return None;
        }
        self.seats.iter().position(|s| s == seat)
    }

    fn changed(&mut self, now: Instant) {
        self.version += 1;
        self.touched = now;
    }

    /// Time left for `side` at `now`.
    fn left(&self, side: usize, now: Instant) -> f64 {
        let Some(c) = &self.clock else { return 0.0 };
        let running = c.since.filter(|_| self.g.turn() == side);
        c.left[side] - running.map(|s| now.saturating_duration_since(s).as_secs_f64() * 1000.0).unwrap_or(0.0)
    }

    /// End the game if the side to move has run out of time.
    fn check_flag(&mut self, now: Instant) {
        let side = self.g.turn();
        let running = self.clock.as_ref().is_some_and(|c| c.since.is_some());
        if !self.end.is_empty() || !running || self.left(side, now) > 0.0 {
            return;
        }
        let can_mate = can_mate(&self.g, side ^ 1);
        if let Some(c) = &mut self.clock {
            c.left[side] = 0.0;
            c.since = None;
        }
        self.end = format!("{}:{side}", if can_mate { "time" } else { "timedraw" });
        self.draw_offer = None;
        self.changed(now);
    }

    fn finish(&mut self, end: String, now: Instant) {
        // Freeze the clocks where they stand.
        let side = self.g.turn();
        let left = self.left(side, now).max(0.0);
        if let Some(c) = &mut self.clock {
            if c.since.take().is_some() {
                c.left[side] = left;
            }
        }
        self.end = end;
        self.draw_offer = None;
        self.changed(now);
    }

    fn json(&self, seat: &str, now: Instant) -> String {
        let moves: Vec<&str> = self.g.history().iter().map(|h| h.san.as_str()).collect();
        let clock = match &self.clock {
            Some(c) => format!(
                "[{:.0},{:.0}],\"inc\":{:.0},\"running\":{}",
                self.left(0, now).max(0.0),
                self.left(1, now).max(0.0),
                c.inc,
                c.since.is_some()
            ),
            None => String::from("null,\"inc\":0,\"running\":false"),
        };
        format!(
            "{{\"v\":{},\"moves\":\"{}\",\"joined\":{},\"end\":\"{}\",\"draw\":{},\"clock\":{clock},\"you\":{}}}",
            self.version,
            moves.join(" "),
            self.joined,
            self.end,
            self.draw_offer.map(|s| s as i32).unwrap_or(-1),
            self.seat_of(seat).unwrap_or(2),
        )
    }
}

/// Whether `side` still has the material to give checkmate at all.
fn can_mate(g: &Engine, side: usize) -> bool {
    let mut minors = 0;
    for &p in g.board().iter().filter(|p| **p != 0 && (**p >> 3) as usize == side) {
        match p & 7 {
            engine::N | engine::B => minors += 1,
            engine::K => {}
            _ => return true,
        }
    }
    minors >= 2
}

fn engine_clock() -> f64 {
    0.0
}
fn engine_rng() -> f64 {
    0.5
}

pub struct Games {
    map: HashMap<String, Game>,
    /// Source of unpredictable numbers for game ids and seat secrets.
    random: Box<dyn FnMut() -> u64 + Send>,
}

impl Games {
    pub fn new(random: Box<dyn FnMut() -> u64 + Send>) -> Games {
        Games { map: HashMap::new(), random }
    }

    fn token(&mut self, len: usize) -> String {
        let mut out = String::with_capacity(len);
        while out.len() < len {
            let mut n = (self.random)();
            for _ in 0..10 {
                if out.len() < len {
                    out.push(ALPHABET[(n % ALPHABET.len() as u64) as usize] as char);
                    n /= ALPHABET.len() as u64;
                }
            }
        }
        out
    }

    fn game(&mut self, id: &str) -> Result<&mut Game, Refusal> {
        self.map.get_mut(id).ok_or(NO_GAME)
    }

    /// Start a game. `color` is "w", "b" or anything else for a coin toss; the clock is
    /// `base_s` seconds plus `inc_s` per move, or none if `base_s` is zero.
    pub fn create(&mut self, color: &str, base_s: f64, inc_s: f64, now: Instant) -> Result<String, Refusal> {
        self.map.retain(|_, g| now.saturating_duration_since(g.touched) < IDLE);
        if self.map.len() >= MAX_GAMES {
            return Err((503, "The server is full. Try again later."));
        }
        if !(0.0..=10_800.0).contains(&base_s) || !(0.0..=60.0).contains(&inc_s) {
            return Err((400, "That time control is not allowed."));
        }
        let creator = match color {
            "w" => 0,
            "b" => 1,
            _ => ((self.random)() & 1) as usize,
        };
        let mut id = self.token(8);
        while self.map.contains_key(&id) {
            id = self.token(8);
        }
        let seats = [self.token(20), self.token(20)];
        let mut g = Engine::new(engine_clock, engine_rng);
        g.load(START);
        let clock = (base_s > 0.0).then(|| Clock { left: [base_s * 1000.0; 2], inc: inc_s * 1000.0, since: None });
        let reply = format!("{{\"id\":\"{id}\",\"seat\":\"{}\",\"color\":{creator}}}", seats[creator]);
        self.map.insert(
            id,
            Game { g, seats, creator, joined: false, version: 1, end: String::new(), draw_offer: None, clock, touched: now },
        );
        Ok(reply)
    }

    /// Take the free seat. Once both are taken, later visitors only watch.
    pub fn join(&mut self, id: &str, now: Instant) -> Result<String, Refusal> {
        let game = self.game(id)?;
        if game.joined {
            return Ok(String::from("{\"seat\":\"\",\"color\":2}"));
        }
        game.joined = true;
        game.changed(now);
        let color = game.creator ^ 1;
        Ok(format!("{{\"seat\":\"{}\",\"color\":{color}}}", game.seats[color]))
    }

    /// The game as `seat` should see it, and its version.
    pub fn state(&mut self, id: &str, seat: &str, now: Instant) -> Result<(u64, String), Refusal> {
        let game = self.game(id)?;
        game.check_flag(now);
        if game.seat_of(seat).is_some() {
            game.touched = now;
        }
        Ok((game.version, game.json(seat, now)))
    }

    /// When the running clock of game `id` reaches zero, if one is running.
    pub fn deadline(&self, id: &str, now: Instant) -> Option<Instant> {
        let game = self.map.get(id)?;
        let c = game.clock.as_ref()?;
        c.since?;
        let left = game.left(game.g.turn(), now).max(0.0);
        Some(now + Duration::from_secs_f64(left / 1000.0))
    }

    /// Play `san` for `seat`. `ply` is how many moves the player had seen, so a move
    /// sent twice, or sent against a position that has moved on, is refused.
    pub fn play(&mut self, id: &str, seat: &str, san: &str, ply: usize, now: Instant) -> Result<String, Refusal> {
        let game = self.game(id)?;
        let who = game.seat_of(seat).ok_or(NOT_PLAYER)?;
        game.check_flag(now);
        if !game.joined {
            return Err((409, "Your friend has not joined yet."));
        }
        if !game.end.is_empty() {
            return Err((409, "The game is over."));
        }
        if ply != game.g.history().len() || game.g.turn() != who {
            return Err((409, "It is not your move."));
        }
        let left = game.left(who, now);
        if game.g.play_san(san).is_none() {
            return Err((400, "That move is not legal."));
        }
        let over = game.g.status().over;
        if let Some(c) = &mut game.clock {
            // The clocks start once White has made the first move.
            if c.since.is_some() {
                c.left[who] = left + c.inc;
            }
            c.since = (!over).then_some(now);
        }
        if over {
            game.end = String::from("board");
        }
        game.draw_offer = None;
        game.changed(now);
        Ok(game.json(seat, now))
    }

    /// `what` is "resign", "offer", "accept", "decline" or "flag".
    pub fn act(&mut self, id: &str, seat: &str, what: &str, now: Instant) -> Result<String, Refusal> {
        let game = self.game(id)?;
        let who = game.seat_of(seat).ok_or(NOT_PLAYER)?;
        game.check_flag(now);
        if game.end.is_empty() && game.joined {
            match what {
                "resign" => game.finish(format!("resign:{who}"), now),
                "offer" if game.draw_offer.is_none() => {
                    game.draw_offer = Some(who);
                    game.changed(now);
                }
                "accept" if game.draw_offer == Some(who ^ 1) => game.finish(String::from("draw"), now),
                "decline" if game.draw_offer == Some(who ^ 1) => {
                    game.draw_offer = None;
                    game.changed(now);
                }
                _ => {}
            }
        }
        Ok(game.json(seat, now))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn games() -> Games {
        let mut n = 0x9e37_79b9_7f4a_7c15u64;
        Games::new(Box::new(move || {
            n = n.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            n >> 11
        }))
    }

    fn field<'a>(json: &'a str, key: &str) -> &'a str {
        let start = json.find(&format!("\"{key}\":")).unwrap_or_else(|| panic!("no {key} in {json}")) + key.len() + 3;
        let rest = &json[start..];
        let end = if let Some(r) = rest.strip_prefix('"') {
            return &r[..r.find('"').unwrap()];
        } else if rest.starts_with('[') {
            rest.find(']').unwrap() + 1
        } else {
            rest.find([',', '}']).unwrap()
        };
        &rest[..end]
    }

    /// A joined game: (games, id, [white seat, black seat]).
    fn started(base: f64, inc: f64, t: Instant) -> (Games, String, [String; 2]) {
        let mut gs = games();
        let made = gs.create("w", base, inc, t).unwrap();
        let id = field(&made, "id").to_string();
        assert_eq!(field(&made, "color"), "0");
        let white = field(&made, "seat").to_string();
        let joined = gs.join(&id, t).unwrap();
        assert_eq!(field(&joined, "color"), "1");
        let black = field(&joined, "seat").to_string();
        assert_ne!(white, black);
        (gs, id, [white, black])
    }

    #[test]
    fn a_game_from_invitation_to_checkmate() {
        let t = Instant::now();
        let mut gs = games();
        let made = gs.create("b", 0.0, 0.0, t).unwrap();
        let id = field(&made, "id").to_string();
        let black = field(&made, "seat").to_string();
        assert_eq!((id.len(), black.len(), field(&made, "color")), (8, 20, "1"));
        // Nobody may move until the second player is there.
        assert_eq!(gs.play(&id, &black, "e5", 0, t).unwrap_err().0, 409);
        let white = field(&gs.join(&id, t).unwrap(), "seat").to_string();
        // A third visitor only watches.
        assert_eq!(gs.join(&id, t).unwrap(), "{\"seat\":\"\",\"color\":2}");
        assert_eq!(field(&gs.state(&id, "", t).unwrap().1, "you"), "2");

        assert_eq!(gs.play(&id, &black, "e5", 0, t).unwrap_err().1, "It is not your move.");
        assert_eq!(gs.play(&id, &white, "Ke2", 0, t).unwrap_err().1, "That move is not legal.");
        assert_eq!(gs.play(&id, "nonsense", "e4", 0, t).unwrap_err().0, 403);
        for (i, san) in ["f3", "e5", "g4"].iter().enumerate() {
            let seat = if i % 2 == 0 { &white } else { &black };
            gs.play(&id, seat, san, i, t).unwrap();
        }
        // The same move sent twice is refused rather than played for the other side.
        assert_eq!(gs.play(&id, &white, "g4", 2, t).unwrap_err().0, 409);
        let end = gs.play(&id, &black, "Qh4#", 3, t).unwrap();
        assert_eq!((field(&end, "moves"), field(&end, "end"), field(&end, "you")), ("f3 e5 g4 Qh4#", "board", "1"));
        assert_eq!(gs.play(&id, &white, "a3", 4, t).unwrap_err().1, "The game is over.");
        assert_eq!(gs.state("missing", "", t).unwrap_err().0, 404);
    }

    #[test]
    fn versions_go_up_only_on_changes() {
        let t = Instant::now();
        let (mut gs, id, [white, black]) = started(0.0, 0.0, t);
        let v = gs.state(&id, &white, t).unwrap().0;
        assert_eq!(gs.state(&id, &black, t).unwrap().0, v);
        gs.play(&id, &white, "e4", 0, t).unwrap();
        assert_eq!(gs.state(&id, &black, t).unwrap().0, v + 1);
    }

    #[test]
    fn resigning_and_agreeing_a_draw() {
        let t = Instant::now();
        let (mut gs, id, [white, black]) = started(0.0, 0.0, t);
        gs.play(&id, &white, "d4", 0, t).unwrap();
        assert_eq!(field(&gs.act(&id, &black, "resign", t).unwrap(), "end"), "resign:1");
        assert_eq!(field(&gs.act(&id, &white, "resign", t).unwrap(), "end"), "resign:1");

        let (mut gs, id, [white, black]) = started(0.0, 0.0, t);
        assert_eq!(field(&gs.act(&id, &white, "offer", t).unwrap(), "draw"), "0");
        // You cannot accept your own offer, and a move withdraws it.
        assert_eq!(field(&gs.act(&id, &white, "accept", t).unwrap(), "end"), "");
        assert_eq!(field(&gs.play(&id, &white, "e4", 0, t).unwrap(), "draw"), "-1");
        gs.act(&id, &black, "offer", t).unwrap();
        assert_eq!(field(&gs.act(&id, &white, "decline", t).unwrap(), "draw"), "-1");
        gs.act(&id, &black, "offer", t).unwrap();
        assert_eq!(field(&gs.act(&id, &white, "accept", t).unwrap(), "end"), "draw");
    }

    #[test]
    fn clocks_run_add_the_increment_and_flag() {
        let t = Instant::now();
        let s = Duration::from_secs;
        let (mut gs, id, [white, black]) = started(60.0, 2.0, t);
        assert_eq!(gs.deadline(&id, t), None);
        // White's first move is free; then Black's clock runs.
        let st = gs.play(&id, &white, "e4", 0, t + s(30)).unwrap();
        assert_eq!((field(&st, "clock"), field(&st, "running"), field(&st, "inc")), ("[60000,60000]", "true", "2000"));
        assert_eq!(gs.deadline(&id, t + s(30)), Some(t + s(90)));
        let st = gs.play(&id, &black, "e5", 1, t + s(40)).unwrap();
        assert_eq!(field(&st, "clock"), "[60000,52000]");
        assert_eq!(field(&gs.state(&id, &white, t + s(45)).unwrap().1, "clock"), "[55000,52000]");
        // White thinks for longer than is left.
        let (_, st) = gs.state(&id, &black, t + s(101)).unwrap();
        assert_eq!((field(&st, "end"), field(&st, "clock"), field(&st, "running")), ("time:0", "[0,52000]", "false"));
        assert_eq!(gs.play(&id, &white, "Nf3", 2, t + s(102)).unwrap_err().1, "The game is over.");
    }

    #[test]
    fn running_out_of_time_against_a_bare_king_is_a_draw() {
        let t = Instant::now();
        let (mut gs, id, [white, _black]) = started(60.0, 0.0, t);
        let game = gs.map.get_mut(&id).unwrap();
        game.g.load("4k3/8/8/8/8/8/4P3/4K3 w - - 0 1");
        gs.play(&id, &white, "e4", 0, t).unwrap();
        // Black, with a bare king, runs out: White has a pawn, so White wins.
        assert_eq!(field(&gs.state(&id, "", t + Duration::from_secs(61)).unwrap().1, "end"), "time:1");

        let (mut gs, id, [white, black]) = started(60.0, 0.0, t);
        gs.map.get_mut(&id).unwrap().g.load("4k3/8/8/8/8/8/4P3/4K3 w - - 0 1");
        gs.play(&id, &white, "e4", 0, t).unwrap();
        gs.play(&id, &black, "Kd7", 1, t).unwrap();
        // White runs out, but Black's lone king could never have won.
        assert_eq!(field(&gs.state(&id, "", t + Duration::from_secs(61)).unwrap().1, "end"), "timedraw:0");
    }

    #[test]
    fn old_games_are_dropped_and_the_server_cannot_be_filled() {
        let t = Instant::now();
        let mut gs = games();
        for _ in 0..MAX_GAMES {
            gs.create("r", 300.0, 0.0, t).unwrap();
        }
        assert_eq!(gs.create("r", 300.0, 0.0, t).unwrap_err().0, 503);
        assert_eq!(gs.create("w", 1e9, 0.0, t + IDLE).unwrap_err().0, 400);
        assert!(gs.create("w", 300.0, 0.0, t + IDLE).is_ok());
        assert_eq!(gs.map.len(), 1);
    }
}
