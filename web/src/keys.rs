//! Playing without a pointer: a keyboard cursor on every board, moves typed as
//! text on the Play board, and spoken announcements for screen readers.

use super::*;

/// Say `text` through the page's live region.
pub fn announce(text: &str) {
    set_text("sr-live", text);
}

/// "e4, white pawn" or "e5, empty".
fn square_words(pos: &[u8; 64], sq: usize) -> String {
    let p = pos[sq];
    if p == 0 {
        return format!("{}, empty", name(sq));
    }
    let kind = ["", "pawn", "knight", "bishop", "rook", "queen", "king"][(p & 7) as usize];
    format!("{}, {} {kind}", name(sq), if p >> 3 == 1 { "black" } else { "white" })
}

impl App {
    /// A key was pressed while board `w` had the focus.
    pub fn board_key(&mut self, w: usize, e: KeyboardEvent) {
        if e.ctrl_key() || e.meta_key() || e.alt_key() {
            return;
        }
        let on_promo_button = e
            .target()
            .and_then(|t| t.dyn_into::<Element>().ok())
            .is_some_and(|t| t.closest("[data-promo]").ok().flatten().is_some());
        let key = e.key();
        // For someone using the mouse the arrow keys keep their page-wide meaning, which is
        // stepping through the moves. The board takes them over once it is being used from
        // the keyboard: reached with Tab, or a square chosen with Enter.
        if !self.boards[w].kbd && !matches!(key.as_str(), "Enter" | " ") {
            return;
        }
        let step = match key.as_str() {
            "ArrowLeft" => Some((-1, 0)),
            "ArrowRight" => Some((1, 0)),
            "ArrowUp" => Some((0, -1)),
            "ArrowDown" => Some((0, 1)),
            _ => None,
        };
        if let Some((dx, dy)) = step {
            // While a promotion is being chosen the arrows have nothing to move.
            if self.boards[w].promo.is_none() {
                let b = &mut self.boards[w];
                // Work in screen coordinates so the arrows mean the same on a flipped board.
                let i = if b.flipped { 63 - b.cursor } else { b.cursor } as i32;
                let (col, row) = ((i & 7) + dx, (i >> 3) + dy);
                let i = (row.clamp(0, 7) * 8 + col.clamp(0, 7)) as usize;
                b.cursor = if b.flipped { 63 - i } else { i };
                b.kbd = true;
                b.render();
                let mut words = square_words(&b.pos, b.cursor);
                if b.targets.iter().any(|m| m.to as usize == b.cursor) {
                    words.push_str(", can move here");
                }
                announce(&words);
            }
        } else if matches!(key.as_str(), "Enter" | " ") {
            if on_promo_button {
                // Let the button's own click choose the piece.
                return;
            }
            if self.boards[w].promo.is_some() {
                return;
            }
            self.boards[w].kbd = true;
            let sq = self.boards[w].cursor;
            if self.board_press(w, sq) {
                let b = &self.boards[w];
                let squares: Vec<String> = b.targets.iter().map(|m| name(m.to as usize)).collect();
                announce(&format!("{} picked up. It can go to {}.", square_words(&b.pos, sq), squares.join(", ")));
            } else {
                self.boards[w].render();
            }
        } else if key == "Escape" {
            let b = &mut self.boards[w];
            b.deselect();
            b.arrows.clear();
            b.render();
            if w == PLAY && self.play.premove.take().is_some() {
                self.play_refresh();
            }
            announce("Cancelled.");
        } else {
            return;
        }
        // Handled here: the page-wide arrow keys (stepping through moves) must not also fire.
        e.prevent_default();
        e.stop_propagation();
    }

    /// Say who moved and what, for people who cannot see the board.
    pub fn play_announce(&self, mover: usize, san: &str) {
        announce(&format!("{} played {san}.", self.play_names()[mover]));
    }

    /// Play the move typed into the box under the Play board.
    pub fn play_typed(&mut self) {
        let input = el("play-san").unchecked_into::<HtmlInputElement>();
        let text = input.value();
        if text.trim().is_empty() {
            return;
        }
        match self.play_find_typed(text.trim()) {
            Ok(m) => {
                input.set_value("");
                set_text("san-msg", "");
                self.boards[PLAY].deselect();
                self.play_on_move(m);
            }
            Err(why) => set_text("san-msg", why),
        }
    }

    /// The legal move meant by `text`: notation such as Nf3 or exd5, or two squares such as e2e4.
    fn play_find_typed(&mut self, text: &str) -> Result<Move, &'static str> {
        let p = &mut self.play;
        if p.status().over {
            return Err("The game is over.");
        }
        if p.view.is_some() {
            return Err("Go to the latest move first.");
        }
        if !p.my_turn() {
            return Err("It is not your move yet.");
        }
        let want: String = text.chars().filter(|c| !"+#!? ".contains(*c)).collect();
        let want = want.replace('0', "O");
        let legal = p.g.legal();
        let named: Vec<(Move, String)> = legal.iter().map(|&m| (m, strip(&p.g.san(m, Some(&legal))))).collect();
        if let Some((m, _)) = named.iter().find(|(_, san)| *san == want) {
            return Ok(*m);
        }
        // Not exact: allow lower case ("nf3") and plain squares ("e2e4", "e7e8q") if only one move fits.
        let lower = text.to_lowercase().replace([' ', '-', '+', '#'], "");
        let fits: Vec<Move> = named
            .iter()
            .filter(|(m, san)| {
                let promo = ["", "", "n", "b", "r", "q", ""][(m.promo & 7) as usize];
                let squares = format!("{}{}{promo}", name(m.from as usize), name(m.to as usize));
                san.to_lowercase().replace('-', "") == lower || squares == lower || (promo == "q" && squares[..4] == lower)
            })
            .map(|(m, _)| *m)
            .collect();
        match fits.as_slice() {
            [m] => Ok(*m),
            [] => Err("That is not a legal move here."),
            _ => Err("More than one move fits that. Use capitals for pieces, as in Bxc3."),
        }
    }
}
