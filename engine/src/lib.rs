//! Chess engine: rules, SAN, FEN, search. Pure Rust with no dependencies, so the
//! same code runs natively (tests) and in WebAssembly (the site and its worker).
//!
//! Squares are 0..64 with a8 = 0 and h1 = 63. A piece is `kind | colour << 3`,
//! where colour 0 is White and 1 is Black.

pub mod pgn;

pub const P: u8 = 1;
pub const N: u8 = 2;
pub const B: u8 = 3;
pub const R: u8 = 4;
pub const Q: u8 = 5;
pub const K: u8 = 6;
pub const MATE: i32 = 100_000;
const INF: i32 = 1_000_000_000;
pub const START: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

const VAL: [i32; 7] = [0, 100, 320, 330, 500, 900, 0];
#[rustfmt::skip]
static PST: [[i32; 64]; 7] = [
    [0; 64],
    [0,0,0,0,0,0,0,0,50,50,50,50,50,50,50,50,10,10,20,30,30,20,10,10,5,5,10,25,25,10,5,5,0,0,0,20,20,0,0,0,5,-5,-10,0,0,-10,-5,5,5,10,10,-20,-20,10,10,5,0,0,0,0,0,0,0,0],
    [-50,-40,-30,-30,-30,-30,-40,-50,-40,-20,0,0,0,0,-20,-40,-30,0,10,15,15,10,0,-30,-30,5,15,20,20,15,5,-30,-30,0,15,20,20,15,0,-30,-30,5,10,15,15,10,5,-30,-40,-20,0,5,5,0,-20,-40,-50,-40,-30,-30,-30,-30,-40,-50],
    [-20,-10,-10,-10,-10,-10,-10,-20,-10,0,0,0,0,0,0,-10,-10,0,5,10,10,5,0,-10,-10,5,5,10,10,5,5,-10,-10,0,10,10,10,10,0,-10,-10,10,10,10,10,10,10,-10,-10,5,0,0,0,0,5,-10,-20,-10,-10,-10,-10,-10,-10,-20],
    [0,0,0,0,0,0,0,0,5,10,10,10,10,10,10,5,-5,0,0,0,0,0,0,-5,-5,0,0,0,0,0,0,-5,-5,0,0,0,0,0,0,-5,-5,0,0,0,0,0,0,-5,-5,0,0,0,0,0,0,-5,0,0,0,5,5,0,0,0],
    [-20,-10,-10,-5,-5,-10,-10,-20,-10,0,0,0,0,0,0,-10,-10,0,5,5,5,5,0,-10,-5,0,5,5,5,5,0,-5,0,0,5,5,5,5,0,-5,-10,5,5,5,5,5,0,-10,-10,0,5,0,0,0,0,-10,-20,-10,-10,-5,-5,-10,-10,-20],
    [-30,-40,-40,-50,-50,-40,-40,-30,-30,-40,-40,-50,-50,-40,-40,-30,-30,-40,-40,-50,-50,-40,-40,-30,-30,-40,-40,-50,-50,-40,-40,-30,-20,-30,-30,-40,-40,-30,-30,-20,-10,-20,-20,-20,-20,-20,-20,-10,20,20,0,0,0,0,20,20,20,30,10,0,0,10,30,20],
];
#[rustfmt::skip]
static KEND: [i32; 64] = [-50,-40,-30,-20,-20,-30,-40,-50,-30,-20,-10,0,0,-10,-20,-30,-30,-10,20,30,30,20,-10,-30,-30,-10,30,40,40,30,-10,-30,-30,-10,30,40,40,30,-10,-30,-30,-10,20,30,30,20,-10,-30,-30,-30,0,0,0,0,-30,-30,-50,-30,-30,-30,-30,-30,-30,-50];
const KN: [(i32, i32); 8] = [(-2, -1), (-2, 1), (-1, -2), (-1, 2), (1, -2), (1, 2), (2, -1), (2, 1)];
const KG: [(i32, i32); 8] = [(-1, -1), (-1, 0), (-1, 1), (0, -1), (0, 1), (1, -1), (1, 0), (1, 1)];
const DIAG: [(i32, i32); 4] = [(-1, -1), (-1, 1), (1, -1), (1, 1)];
const ORTH: [(i32, i32); 4] = [(-1, 0), (1, 0), (0, -1), (0, 1)];
const LET: [char; 7] = [' ', 'P', 'N', 'B', 'R', 'Q', 'K'];
const FILES: [char; 8] = ['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h'];

/// Castling rights that survive a move touching this square.
fn castle_mask(sq: usize) -> u8 {
    match sq {
        60 => 12,
        63 => 14,
        56 => 13,
        4 => 3,
        7 => 11,
        0 => 7,
        _ => 15,
    }
}

/// Flags: 1 double pawn push, 2 en passant, 3 castle kingside, 4 castle queenside.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Move {
    pub from: u8,
    pub to: u8,
    pub piece: u8,
    pub cap: u8,
    pub promo: u8,
    pub flag: u8,
}

#[derive(Clone, Debug)]
pub struct Played {
    pub m: Move,
    pub san: String,
}

#[derive(Clone, Debug, Default)]
pub struct Status {
    pub over: bool,
    pub mate: bool,
    pub text: String,
}

#[derive(Clone, Copy, Debug)]
pub struct SearchOpts {
    pub max_depth: i32,
    /// Random centipawn noise added to root scores; makes weaker levels fallible.
    pub noise: i32,
    pub time_ms: f64,
}

impl Default for SearchOpts {
    fn default() -> Self {
        SearchOpts { max_depth: 3, noise: 0, time_ms: 8000.0 }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchOut {
    pub from: u8,
    pub to: u8,
    pub promo: u8,
    /// Centipawns from the point of view of the side to move.
    pub score: i32,
    pub san: String,
    pub depth: i32,
}

pub fn name(sq: usize) -> String {
    format!("{}{}", FILES[sq & 7], 8 - (sq >> 3))
}

pub fn sq_of(n: &str) -> usize {
    let b = n.as_bytes();
    (8 - (b[1] - b'0') as usize) * 8 + (b[0] - b'a') as usize
}

/// Check that `fen` describes a position a game could be played from, so that a
/// pasted string can be loaded safely. The error says what is wrong in plain words.
pub fn validate_fen(fen: &str) -> Result<(), String> {
    let f: Vec<&str> = fen.split_whitespace().collect();
    if f.len() < 2 || f.len() > 6 {
        return Err("A FEN has up to six parts: the board, whose move it is, castling, en passant and two counters.".into());
    }
    let ranks: Vec<&str> = f[0].split('/').collect();
    if ranks.len() != 8 {
        return Err("The board part of the FEN must have eight ranks separated by '/'.".into());
    }
    let (mut wk, mut bk) = (0, 0);
    for (i, rank) in ranks.iter().enumerate() {
        let mut width = 0;
        for ch in rank.chars() {
            match ch {
                '1'..='8' => width += ch.to_digit(10).unwrap_or(0),
                'p' | 'P' if i == 0 || i == 7 => return Err("Pawns cannot stand on the first or last rank.".into()),
                'p' | 'n' | 'b' | 'r' | 'q' | 'P' | 'N' | 'B' | 'R' | 'Q' => width += 1,
                'K' => {
                    wk += 1;
                    width += 1
                }
                'k' => {
                    bk += 1;
                    width += 1
                }
                _ => return Err(format!("'{ch}' is not a piece.")),
            }
        }
        if width != 8 {
            return Err(format!("Rank {} of the FEN does not add up to eight squares.", 8 - i));
        }
    }
    if wk != 1 || bk != 1 {
        return Err("Each side needs exactly one king.".into());
    }
    if f[1] != "w" && f[1] != "b" {
        return Err("The second part of the FEN must be 'w' or 'b' for the side to move.".into());
    }
    if let Some(c) = f.get(2) {
        if *c != "-" && !c.chars().all(|ch| "KQkq".contains(ch)) {
            return Err("The castling part of the FEN may only contain K, Q, k, q or '-'.".into());
        }
    }
    if let Some(e) = f.get(3) {
        let b = e.as_bytes();
        if *e != "-" && !(b.len() == 2 && (b'a'..=b'h').contains(&b[0]) && (b[1] == b'3' || b[1] == b'6')) {
            return Err("The en passant part of the FEN must be a square on the third or sixth rank, or '-'.".into());
        }
    }
    for n in f.iter().skip(4) {
        if n.parse::<u32>().is_err() {
            return Err("The move counters at the end of the FEN must be numbers.".into());
        }
    }
    let mut e = Engine::new(|| 0.0, || 0.0);
    e.load(fen);
    if e.attacked(e.king_sq[e.turn ^ 1], e.turn) {
        return Err("The side that just moved is in check, which cannot happen in a real game.".into());
    }
    Ok(())
}

fn on_board(r: i32, f: i32) -> bool {
    (0..8).contains(&r) && (0..8).contains(&f)
}

pub struct Engine {
    board: [u8; 64],
    turn: usize,
    castling: u8,
    ep: i32,
    half: u32,
    full: u32,
    king_sq: [usize; 2],
    undo: Vec<(u8, i32, u32, usize, usize)>,
    hist: Vec<Played>,
    keys: Vec<String>,
    nodes: u64,
    stop_at: f64,
    stopped: bool,
    clock: fn() -> f64,
    rng: fn() -> f64,
}

impl Engine {
    /// `clock` returns milliseconds; `rng` returns a uniform value in [0, 1).
    /// They are injected because wasm32 has no std clock or randomness.
    pub fn new(clock: fn() -> f64, rng: fn() -> f64) -> Engine {
        let mut e = Engine {
            board: [0; 64],
            turn: 0,
            castling: 0,
            ep: -1,
            half: 0,
            full: 1,
            king_sq: [60, 4],
            undo: Vec::new(),
            hist: Vec::new(),
            keys: Vec::new(),
            nodes: 0,
            stop_at: 0.0,
            stopped: false,
            clock,
            rng,
        };
        e.load(START);
        e
    }

    pub fn load(&mut self, fen: &str) {
        let p: Vec<&str> = fen.split_whitespace().collect();
        self.board = [0; 64];
        let mut s = 0usize;
        for ch in p.first().copied().unwrap_or("").chars() {
            if ch == '/' {
                continue;
            }
            if let Some(d) = ch.to_digit(10) {
                s += d as usize;
                continue;
            }
            let t = LET.iter().position(|&l| l == ch.to_ascii_uppercase()).unwrap_or(0) as u8;
            if s < 64 {
                let c = if ch.is_ascii_lowercase() { 1 } else { 0 };
                self.board[s] = t | (c << 3);
                if t == K {
                    self.king_sq[c as usize] = s;
                }
            }
            s += 1;
        }
        self.turn = if p.get(1) == Some(&"b") { 1 } else { 0 };
        let c = p.get(2).copied().unwrap_or("-");
        self.castling = 0;
        for (ch, bit) in [('K', 1), ('Q', 2), ('k', 4), ('q', 8)] {
            if c.contains(ch) {
                self.castling |= bit;
            }
        }
        self.ep = match p.get(3) {
            Some(&e) if e != "-" && e.len() == 2 => sq_of(e) as i32,
            _ => -1,
        };
        self.half = p.get(4).and_then(|x| x.parse().ok()).unwrap_or(0);
        self.full = p.get(5).and_then(|x| x.parse().ok()).unwrap_or(1);
        self.undo.clear();
        self.hist.clear();
        self.keys = vec![self.key()];
    }

    /// Position identity for repetition: FEN without the move counters.
    fn key(&self) -> String {
        let mut s = String::with_capacity(80);
        for r in 0..8 {
            let mut e = 0;
            for f in 0..8 {
                let p = self.board[r * 8 + f];
                if p == 0 {
                    e += 1;
                    continue;
                }
                if e > 0 {
                    s.push_str(&e.to_string());
                    e = 0;
                }
                let l = LET[(p & 7) as usize];
                s.push(if p >> 3 == 1 { l.to_ascii_lowercase() } else { l });
            }
            if e > 0 {
                s.push_str(&e.to_string());
            }
            if r < 7 {
                s.push('/');
            }
        }
        s.push(' ');
        s.push(if self.turn == 1 { 'b' } else { 'w' });
        s.push(' ');
        if self.castling == 0 {
            s.push('-');
        } else {
            for (ch, bit) in [('K', 1), ('Q', 2), ('k', 4), ('q', 8)] {
                if self.castling & bit != 0 {
                    s.push(ch);
                }
            }
        }
        s.push(' ');
        if self.ep >= 0 {
            s.push_str(&name(self.ep as usize));
        } else {
            s.push('-');
        }
        s
    }

    pub fn fen(&self) -> String {
        format!("{} {} {}", self.key(), self.half, self.full)
    }
    pub fn board(&self) -> &[u8; 64] {
        &self.board
    }
    pub fn turn(&self) -> usize {
        self.turn
    }
    pub fn king_sq(&self, c: usize) -> usize {
        self.king_sq[c]
    }
    pub fn history(&self) -> &[Played] {
        &self.hist
    }

    fn at(&self, r: i32, f: i32) -> u8 {
        self.board[(r * 8 + f) as usize]
    }

    fn attacked(&self, sq: usize, by: usize) -> bool {
        let r = (sq >> 3) as i32;
        let f = (sq & 7) as i32;
        let c = (by as u8) << 3;
        let pr = if by == 0 { r + 1 } else { r - 1 };
        if (0..8).contains(&pr) {
            if f > 0 && self.at(pr, f - 1) == (P | c) {
                return true;
            }
            if f < 7 && self.at(pr, f + 1) == (P | c) {
                return true;
            }
        }
        for i in 0..8 {
            let (rr, ff) = (r + KN[i].0, f + KN[i].1);
            if on_board(rr, ff) && self.at(rr, ff) == (N | c) {
                return true;
            }
            let (rr, ff) = (r + KG[i].0, f + KG[i].1);
            if on_board(rr, ff) && self.at(rr, ff) == (K | c) {
                return true;
            }
        }
        for (dirs, slider) in [(&DIAG, B), (&ORTH, R)] {
            for d in dirs.iter() {
                let (mut rr, mut ff) = (r + d.0, f + d.1);
                while on_board(rr, ff) {
                    let p = self.at(rr, ff);
                    if p != 0 {
                        if (p >> 3) as usize == by && ((p & 7) == slider || (p & 7) == Q) {
                            return true;
                        }
                        break;
                    }
                    rr += d.0;
                    ff += d.1;
                }
            }
        }
        false
    }

    pub fn in_check(&self) -> bool {
        self.attacked(self.king_sq[self.turn], self.turn ^ 1)
    }

    /// Pseudo-legal moves; the caller filters out those that leave the king in check.
    fn gen(&self, caps_only: bool) -> Vec<Move> {
        let mut out: Vec<Move> = Vec::with_capacity(48);
        let us = self.turn;
        let them = (us ^ 1) as u8;
        let b = &self.board;
        for s in 0..64usize {
            let p = b[s];
            if p == 0 || (p >> 3) as usize != us {
                continue;
            }
            let t = p & 7;
            let r = (s >> 3) as i32;
            let f = (s & 7) as i32;
            let mut add = |to: usize, cap: u8, promo: u8, flag: u8| {
                out.push(Move { from: s as u8, to: to as u8, piece: p, cap, promo, flag })
            };
            if t == P {
                let d: i32 = if us == 0 { -1 } else { 1 };
                let r1 = r + d;
                let last = if us == 0 { 0 } else { 7 };
                let home = if us == 0 { 6 } else { 1 };
                if !(0..8).contains(&r1) {
                    continue;
                }
                let mut add_p = |to: usize, cap: u8| {
                    if r1 == last {
                        for pr in [Q, R, B, N] {
                            add(to, cap, pr, 0);
                        }
                    } else {
                        add(to, cap, 0, 0);
                    }
                };
                let to = (r1 * 8 + f) as usize;
                if b[to] == 0 && (!caps_only || r1 == last) {
                    add_p(to, 0);
                }
                for df in [-1, 1] {
                    let f1 = f + df;
                    if !(0..8).contains(&f1) {
                        continue;
                    }
                    let to = (r1 * 8 + f1) as usize;
                    let c = b[to];
                    if c != 0 {
                        if c >> 3 == them {
                            add_p(to, c);
                        }
                    }
                }
                // Double push and en passant are never promotions.
                if b[to] == 0 && !caps_only && r == home {
                    let to2 = (to as i32 + d * 8) as usize;
                    if b[to2] == 0 {
                        add(to2, 0, 0, 1);
                    }
                }
                for df in [-1, 1] {
                    let f1 = f + df;
                    if !(0..8).contains(&f1) {
                        continue;
                    }
                    let to = r1 * 8 + f1;
                    if b[to as usize] == 0 && to == self.ep {
                        add(to as usize, P | (them << 3), 0, 2);
                    }
                }
            } else if t == N || t == K {
                let dirs = if t == N { &KN } else { &KG };
                for d in dirs.iter() {
                    let (rr, ff) = (r + d.0, f + d.1);
                    if !on_board(rr, ff) {
                        continue;
                    }
                    let to = (rr * 8 + ff) as usize;
                    let c = b[to];
                    if c == 0 {
                        if !caps_only {
                            add(to, 0, 0, 0);
                        }
                    } else if c >> 3 == them {
                        add(to, c, 0, 0);
                    }
                }
                if t == K && !caps_only {
                    if us == 0 && s == 60 {
                        if self.castling & 1 != 0 && b[61] == 0 && b[62] == 0 && b[63] == R
                            && !self.attacked(60, 1) && !self.attacked(61, 1) && !self.attacked(62, 1)
                        {
                            add(62, 0, 0, 3);
                        }
                        if self.castling & 2 != 0 && b[59] == 0 && b[58] == 0 && b[57] == 0 && b[56] == R
                            && !self.attacked(60, 1) && !self.attacked(59, 1) && !self.attacked(58, 1)
                        {
                            add(58, 0, 0, 4);
                        }
                    } else if us == 1 && s == 4 {
                        if self.castling & 4 != 0 && b[5] == 0 && b[6] == 0 && b[7] == (R | 8)
                            && !self.attacked(4, 0) && !self.attacked(5, 0) && !self.attacked(6, 0)
                        {
                            add(6, 0, 0, 3);
                        }
                        if self.castling & 8 != 0 && b[3] == 0 && b[2] == 0 && b[1] == 0 && b[0] == (R | 8)
                            && !self.attacked(4, 0) && !self.attacked(3, 0) && !self.attacked(2, 0)
                        {
                            add(2, 0, 0, 4);
                        }
                    }
                }
            } else {
                for (k, dirs) in [&DIAG, &ORTH].into_iter().enumerate() {
                    if (k == 0 && t == R) || (k == 1 && t == B) {
                        continue;
                    }
                    for d in dirs.iter() {
                        let (mut rr, mut ff) = (r + d.0, f + d.1);
                        while on_board(rr, ff) {
                            let to = (rr * 8 + ff) as usize;
                            let c = b[to];
                            if c == 0 {
                                if !caps_only {
                                    add(to, 0, 0, 0);
                                }
                            } else {
                                if c >> 3 == them {
                                    add(to, c, 0, 0);
                                }
                                break;
                            }
                            rr += d.0;
                            ff += d.1;
                        }
                    }
                }
            }
        }
        out
    }

    fn make(&mut self, m: Move) {
        let us = self.turn;
        let (from, to) = (m.from as usize, m.to as usize);
        self.undo.push((self.castling, self.ep, self.half, self.king_sq[0], self.king_sq[1]));
        self.board[from] = 0;
        self.board[to] = if m.promo != 0 { m.promo | ((us as u8) << 3) } else { m.piece };
        match m.flag {
            2 => self.board[if us == 0 { to + 8 } else { to - 8 }] = 0,
            3 => {
                self.board[to - 1] = self.board[to + 1];
                self.board[to + 1] = 0;
            }
            4 => {
                self.board[to + 1] = self.board[to - 2];
                self.board[to - 2] = 0;
            }
            _ => {}
        }
        if m.piece & 7 == K {
            self.king_sq[us] = to;
        }
        self.castling &= castle_mask(from) & castle_mask(to);
        self.ep = if m.flag == 1 { ((from + to) >> 1) as i32 } else { -1 };
        self.half = if m.piece & 7 == P || m.cap != 0 { 0 } else { self.half + 1 };
        if us == 1 {
            self.full += 1;
        }
        self.turn = us ^ 1;
    }

    fn unmake(&mut self, m: Move) {
        self.turn ^= 1;
        let us = self.turn;
        let (from, to) = (m.from as usize, m.to as usize);
        if us == 1 {
            self.full -= 1;
        }
        let (castling, ep, half, wk, bk) = self.undo.pop().expect("unmake without make");
        self.castling = castling;
        self.ep = ep;
        self.half = half;
        self.king_sq = [wk, bk];
        self.board[from] = m.piece;
        self.board[to] = 0;
        if m.flag == 2 {
            self.board[if us == 0 { to + 8 } else { to - 8 }] = m.cap;
        } else {
            self.board[to] = m.cap;
            if m.flag == 3 {
                self.board[to + 1] = self.board[to - 1];
                self.board[to - 1] = 0;
            } else if m.flag == 4 {
                self.board[to - 2] = self.board[to + 1];
                self.board[to + 1] = 0;
            }
        }
    }

    pub fn legal(&mut self) -> Vec<Move> {
        let us = self.turn;
        let mut out = Vec::new();
        for m in self.gen(false) {
            self.make(m);
            if !self.attacked(self.king_sq[us], us ^ 1) {
                out.push(m);
            }
            self.unmake(m);
        }
        out
    }

    /// Standard algebraic notation. `list` is the legal move list if the caller has it.
    pub fn san(&mut self, m: Move, list: Option<&[Move]>) -> String {
        let mut s;
        if m.flag == 3 {
            s = String::from("O-O");
        } else if m.flag == 4 {
            s = String::from("O-O-O");
        } else {
            let t = m.piece & 7;
            s = String::new();
            if t == P {
                if m.cap != 0 {
                    s.push(FILES[(m.from & 7) as usize]);
                    s.push('x');
                }
                s.push_str(&name(m.to as usize));
                if m.promo != 0 {
                    s.push('=');
                    s.push(LET[m.promo as usize]);
                }
            } else {
                let owned;
                let all: &[Move] = match list {
                    Some(l) => l,
                    None => {
                        owned = self.legal();
                        &owned
                    }
                };
                let others: Vec<&Move> =
                    all.iter().filter(|o| o.piece == m.piece && o.to == m.to && o.from != m.from).collect();
                s.push(LET[t as usize]);
                if !others.is_empty() {
                    if !others.iter().any(|o| o.from & 7 == m.from & 7) {
                        s.push(FILES[(m.from & 7) as usize]);
                    } else if !others.iter().any(|o| o.from >> 3 == m.from >> 3) {
                        s.push_str(&(8 - (m.from >> 3)).to_string());
                    } else {
                        s.push_str(&name(m.from as usize));
                    }
                }
                if m.cap != 0 {
                    s.push('x');
                }
                s.push_str(&name(m.to as usize));
            }
        }
        self.make(m);
        if self.in_check() {
            s.push(if self.legal().is_empty() { '#' } else { '+' });
        }
        self.unmake(m);
        s
    }

    // ---- evaluation and search ----

    fn evaluate(&self) -> i32 {
        let mut mg = 0;
        let mut npm = 0;
        let mut mat = [0i32; 2];
        for s in 0..64usize {
            let p = self.board[s];
            if p == 0 {
                continue;
            }
            let t = (p & 7) as usize;
            let c = (p >> 3) as usize;
            if t as u8 == K {
                continue;
            }
            let v = VAL[t] + PST[t][if c == 1 { s ^ 56 } else { s }];
            mg += if c == 1 { -v } else { v };
            if t as u8 != P {
                npm += VAL[t];
            }
            mat[c] += VAL[t];
        }
        let end = npm <= 2700;
        let (wk, bk) = (self.king_sq[0], self.king_sq[1]);
        let kt: &[i32; 64] = if end { &KEND } else { &PST[K as usize] };
        mg += kt[wk] - kt[bk ^ 56];
        if end {
            // With a decisive material edge, push the losing king to the rim and walk ours closer.
            let diff = mat[0] - mat[1];
            if diff.abs() >= 400 {
                let lk = if diff > 0 { bk } else { wk };
                let (r, f) = ((lk >> 3) as i32, (lk & 7) as i32);
                let edge = (3 - r).max(r - 4) + (3 - f).max(f - 4);
                let dist = ((wk >> 3) as i32 - (bk >> 3) as i32).abs() + ((wk & 7) as i32 - (bk & 7) as i32).abs();
                let bonus = edge * 12 + (14 - dist) * 5;
                mg += if diff > 0 { bonus } else { -bonus };
            }
        }
        if self.turn == 0 {
            mg
        } else {
            -mg
        }
    }

    fn order(ms: &mut [Move]) {
        ms.sort_by_key(|m| {
            -(if m.cap != 0 {
                10000 + 10 * VAL[(m.cap & 7) as usize] - VAL[(m.piece & 7) as usize]
            } else if m.promo != 0 {
                9000
            } else {
                0
            })
        });
    }

    fn quiesce(&mut self, mut alpha: i32, beta: i32, ply: i32) -> i32 {
        self.nodes += 1;
        let stand = self.evaluate();
        if stand >= beta {
            return beta;
        }
        if stand > alpha {
            alpha = stand;
        }
        if ply > 40 {
            return alpha;
        }
        let us = self.turn;
        let mut ms = self.gen(true);
        Self::order(&mut ms);
        for m in ms {
            self.make(m);
            if self.attacked(self.king_sq[us], us ^ 1) {
                self.unmake(m);
                continue;
            }
            let sc = -self.quiesce(-beta, -alpha, ply + 1);
            self.unmake(m);
            if sc >= beta {
                return beta;
            }
            if sc > alpha {
                alpha = sc;
            }
        }
        alpha
    }

    fn negamax(&mut self, mut depth: i32, mut alpha: i32, beta: i32, ply: i32) -> i32 {
        self.nodes += 1;
        if self.nodes & 1023 == 0 && (self.clock)() > self.stop_at {
            self.stopped = true;
        }
        if self.stopped || self.half >= 100 {
            return 0;
        }
        let chk = self.in_check();
        if chk && ply < 30 {
            depth += 1;
        }
        if depth <= 0 {
            return self.quiesce(alpha, beta, ply);
        }
        let us = self.turn;
        let mut ms = self.gen(false);
        Self::order(&mut ms);
        let mut n = 0;
        for m in ms {
            self.make(m);
            if self.attacked(self.king_sq[us], us ^ 1) {
                self.unmake(m);
                continue;
            }
            n += 1;
            let sc = -self.negamax(depth - 1, -beta, -alpha, ply + 1);
            self.unmake(m);
            if self.stopped {
                return 0;
            }
            if sc >= beta {
                return beta;
            }
            if sc > alpha {
                alpha = sc;
            }
        }
        if n == 0 {
            return if chk { -MATE + ply } else { 0 };
        }
        alpha
    }

    /// Iterative-deepening alpha-beta. Returns `None` when there is no legal move.
    pub fn search(&mut self, o: SearchOpts) -> Option<SearchOut> {
        let mut legal = self.legal();
        if legal.is_empty() {
            return None;
        }
        Self::order(&mut legal);
        let mut ms: Vec<(Move, Option<i32>)> = legal.into_iter().map(|m| (m, None)).collect();
        self.nodes = 0;
        self.stopped = false;
        self.stop_at = (self.clock)() + o.time_ms;
        let mut best = ms[0].0;
        let mut best_score = 0;
        let mut done = 0;
        // Root scores from the deepest fully searched iteration, used for noise.
        let mut table: Option<Vec<(Move, Option<i32>)>> = None;
        for d in 1..=o.max_depth.max(1) {
            let mut alpha = -INF;
            let mut lb: Option<Move> = None;
            let mut ls = -INF;
            for e in ms.iter_mut() {
                e.1 = None;
            }
            for i in 0..ms.len() {
                let m = ms[i].0;
                self.make(m);
                // With noise every root move needs an exact score, so no root cutoffs.
                let s = -self.negamax(d - 1, -INF, if o.noise != 0 { INF } else { -alpha }, 1);
                self.unmake(m);
                if self.stopped {
                    break;
                }
                ms[i].1 = Some(s);
                if s > ls {
                    ls = s;
                    lb = Some(m);
                }
                if s > alpha {
                    alpha = s;
                }
            }
            if self.stopped && done > 0 {
                break;
            }
            let Some(lb) = lb else { break };
            best = lb;
            best_score = ls;
            done = d;
            if !self.stopped {
                table = Some(ms.clone());
            }
            ms.sort_by_key(|e| -(e.1.unwrap_or(-INF) as i64));
            if self.stopped || ls.abs() > MATE - 200 {
                break;
            }
        }
        if o.noise != 0 && best_score.abs() < MATE - 200 {
            if let Some(t) = &table {
                let mut top = f64::NEG_INFINITY;
                for (m, s) in t {
                    if let Some(s) = s {
                        let v = *s as f64 + ((self.rng)() - 0.5) * 2.0 * o.noise as f64;
                        if v > top {
                            top = v;
                            best = *m;
                        }
                    }
                }
            }
        }
        let san = self.san(best, None);
        Some(SearchOut { from: best.from, to: best.to, promo: best.promo, score: best_score, san, depth: done })
    }

    // ---- forced mate helpers for puzzles ----

    /// True if the side to move is checkmated now or cannot avoid mate within `k` attacker moves.
    pub fn mated_within(&mut self, k: i32) -> bool {
        let ms = self.legal();
        if ms.is_empty() {
            return self.in_check();
        }
        if k <= 0 {
            return false;
        }
        for m in ms {
            self.make(m);
            let r = self.find_mate(k).is_some();
            self.unmake(m);
            if !r {
                return false;
            }
        }
        true
    }

    pub fn find_mate(&mut self, n: i32) -> Option<Move> {
        for m in self.legal() {
            self.make(m);
            let r = self.mated_within(n - 1);
            self.unmake(m);
            if r {
                return Some(m);
            }
        }
        None
    }

    // ---- game layer ----

    pub fn play(&mut self, m: Move) -> String {
        let s = self.san(m, None);
        self.make(m);
        self.hist.push(Played { m, san: s.clone() });
        self.keys.push(self.key());
        s
    }

    pub fn takeback(&mut self) -> Option<Played> {
        let h = self.hist.pop()?;
        self.keys.pop();
        self.unmake(h.m);
        Some(h)
    }

    pub fn play_san(&mut self, s: &str) -> Option<Move> {
        let want: String = s.chars().filter(|c| !"+#!?".contains(*c)).collect();
        let ms = self.legal();
        let m = ms.iter().copied().find(|&x| strip(&self.san(x, Some(&ms))) == want)?;
        self.play(m);
        Some(m)
    }

    pub fn status(&mut self) -> Status {
        let draw = |text: &str| Status { over: true, mate: false, text: text.to_string() };
        if self.legal().is_empty() {
            return if self.in_check() {
                let winner = if self.turn == 1 { "White" } else { "Black" };
                Status { over: true, mate: true, text: format!("Checkmate. {winner} wins.") }
            } else {
                draw("Stalemate. The game is a draw.")
            };
        }
        if self.half >= 100 {
            return draw("Draw by the fifty-move rule.");
        }
        if let Some(k) = self.keys.last() {
            if self.keys.iter().filter(|x| *x == k).count() >= 3 {
                return draw("Draw by threefold repetition.");
            }
        }
        let (mut minors, mut other) = (0, 0);
        for &p in self.board.iter() {
            let t = p & 7;
            if t == N || t == B {
                minors += 1;
            } else if t != 0 && t != K {
                other += 1;
            }
        }
        if other == 0 && minors <= 1 {
            return draw("Draw. Neither side can checkmate.");
        }
        Status::default()
    }

    pub fn perft(&mut self, d: u32) -> u64 {
        if d == 0 {
            return 1;
        }
        let mut n = 0;
        for m in self.legal() {
            self.make(m);
            n += self.perft(d - 1);
            self.unmake(m);
        }
        n
    }
}

/// Drop check and mate marks from a SAN string.
pub fn strip(s: &str) -> String {
    s.chars().filter(|c| *c != '+' && *c != '#').collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eng() -> Engine {
        Engine::new(|| 0.0, || 0.5)
    }

    #[test]
    fn perft_start() {
        let mut e = eng();
        assert_eq!(e.perft(1), 20);
        assert_eq!(e.perft(2), 400);
        assert_eq!(e.perft(3), 8902);
        assert_eq!(e.perft(4), 197_281);
    }

    #[test]
    fn perft_kiwipete() {
        // Exercises castling, en passant, promotions and pins.
        let mut e = eng();
        e.load("r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1");
        assert_eq!(e.perft(1), 48);
        assert_eq!(e.perft(2), 2039);
        assert_eq!(e.perft(3), 97_862);
    }

    #[test]
    fn perft_promotions_and_ep() {
        let mut e = eng();
        e.load("8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1");
        assert_eq!(e.perft(4), 43_238);
        e.load("rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8");
        assert_eq!(e.perft(3), 62_379);
    }

    #[test]
    fn fen_round_trip() {
        let mut e = eng();
        assert_eq!(e.fen(), START);
        for s in ["e4", "c5", "Nf3"] {
            assert!(e.play_san(s).is_some(), "{s}");
        }
        assert_eq!(e.fen(), "rnbqkbnr/pp1ppppp/8/2p5/4P3/5N2/PPPP1PPP/RNBQKB1R b KQkq - 1 2");
        e.takeback();
        e.takeback();
        e.takeback();
        assert_eq!(e.fen(), START);
    }

    #[test]
    fn san_and_mate() {
        let mut e = eng();
        for s in ["f3", "e5", "g4"] {
            e.play_san(s).unwrap();
        }
        let m = e.play_san("Qh4").unwrap();
        assert_eq!(e.history().last().unwrap().san, "Qh4#");
        assert_eq!(m.to as usize, sq_of("h4"));
        let st = e.status();
        assert!(st.mate && st.text == "Checkmate. Black wins.");
    }

    #[test]
    fn fen_validation() {
        assert!(validate_fen(START).is_ok());
        assert!(validate_fen("4k3/8/8/8/8/8/8/4K3 w - -").is_ok());
        assert!(validate_fen("4k3/8/8/8/8/8/8/4K3 b").is_ok());
        for bad in [
            "",
            "hello world",
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBN w KQkq - 0 1",  // rank of seven
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP w KQkq - 0 1",          // seven ranks
            "rnbq1bnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1", // no black king
            "4k3/8/8/8/8/8/8/3KK3 w - - 0 1",                           // two white kings
            "P3k3/8/8/8/8/8/8/4K3 w - - 0 1",                           // pawn on the eighth
            "4k3/8/8/8/8/8/8/4K3 x - - 0 1",                            // bad side
            "4k3/8/8/8/8/8/8/4K3 w KX - 0 1",                           // bad castling
            "4k3/8/8/8/8/8/8/4K3 w - e4 0 1",                           // bad en passant
            "4k3/8/8/8/8/8/8/4K3 w - - x 1",                            // bad counter
            "4k3/8/8/8/8/8/8/4K2r b - - 0 1",                           // white already in check, black to move
        ] {
            assert!(validate_fen(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn pgn_round_trip_through_the_engine() {
        let mut e = eng();
        for s in ["e4", "e5", "Nf3", "Nc6", "Bc4", "Bc5", "O-O", "Nf6"] {
            e.play_san(s).unwrap();
        }
        let sans: Vec<String> = e.history().iter().map(|h| h.san.clone()).collect();
        let text = pgn::format(&[("Event", "Test")], None, &sans, "*");
        let back = pgn::parse(&text);
        let mut f = eng();
        for s in &back.moves {
            assert!(f.play_san(s).is_some(), "{s}");
        }
        assert_eq!(e.fen(), f.fen());
    }

    #[test]
    fn search_finds_mates() {
        let mut e = eng();
        e.load("6k1/5ppp/8/8/8/8/5PPP/3R2K1 w - - 0 1");
        let r = e.search(SearchOpts { max_depth: 3, noise: 0, time_ms: 1e12 }).unwrap();
        assert_eq!(r.san, "Rd8#");
        e.load("6k1/8/8/8/8/8/R7/1R4K1 w - - 0 1");
        assert!(e.find_mate(1).is_none());
        assert!(e.find_mate(2).is_some());
    }
}
