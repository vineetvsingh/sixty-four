//! Checks the site's lessons, openings and puzzles against the engine, so a typo in a
//! position or a move is caught here rather than by a visitor.

#[allow(dead_code)]
#[path = "../../web/src/content.rs"]
mod content;

use content::{LESSONS, OPENINGS, PUZZLES};
use engine::{strip, validate_fen, Engine, SearchOpts, START};
use std::collections::HashSet;
use std::sync::OnceLock;
use std::time::Instant;

fn clock() -> f64 {
    static START_TIME: OnceLock<Instant> = OnceLock::new();
    START_TIME.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}
fn rng() -> f64 {
    0.5
}
fn at(fen: &str) -> Engine {
    validate_fen(fen).unwrap_or_else(|why| panic!("{fen}: {why}"));
    let mut g = Engine::new(clock, rng);
    g.load(fen);
    g
}

#[test]
fn ids_are_unique() {
    let lessons: HashSet<_> = LESSONS.iter().map(|l| l.id).collect();
    let puzzles: HashSet<_> = PUZZLES.iter().map(|p| p.id).collect();
    assert_eq!((lessons.len(), puzzles.len()), (LESSONS.len(), PUZZLES.len()));
}

#[test]
fn lesson_positions_are_valid_and_their_answers_are_legal() {
    for l in LESSONS {
        let mut g = at(l.fen.unwrap_or(START));
        let Some(task) = &l.task else { continue };
        let legal = g.legal();
        let sans: Vec<String> = legal.iter().map(|&m| strip(&g.san(m, Some(&legal)))).collect();
        for want in task.accept {
            assert!(sans.iter().any(|s| s == want), "lesson {}: {want} is not legal (legal: {sans:?})", l.id);
        }
    }
}

#[test]
fn opening_lines_are_legal_and_notes_point_at_real_moves() {
    for o in OPENINGS {
        let mut g = at(START);
        let moves: Vec<&str> = o.moves.split(' ').collect();
        for (i, san) in moves.iter().enumerate() {
            assert!(g.play_san(san).is_some(), "{}: move {} ({san}) is not legal", o.name, i + 1);
        }
        for (ply, _) in o.notes {
            assert!(*ply <= moves.len(), "{}: note for ply {ply}, but the line has {} moves", o.name, moves.len());
        }
        assert!(!o.white.is_empty() && !o.black.is_empty() && o.eco.len() == 3, "{}", o.name);
    }
}

#[test]
fn mate_puzzles_are_mate_in_exactly_the_stated_number() {
    for p in PUZZLES.iter().filter(|p| p.mate > 0) {
        let mut g = at(p.fen);
        assert!(!g.status().over, "{}: the game is already over", p.id);
        assert!(g.find_mate(p.mate).is_some(), "{}: no forced mate in {}", p.id, p.mate);
        if p.mate > 1 {
            assert!(g.find_mate(p.mate - 1).is_none(), "{}: there is a quicker mate than {}", p.id, p.mate);
        }
    }
}

#[test]
fn tactics_lines_are_legal_and_start_with_the_best_move() {
    for p in PUZZLES.iter().filter(|p| p.mate == 0) {
        let mut g = at(p.fen);
        assert!(p.line.len() % 2 == 1, "{}: the line must end on the solver's move", p.id);
        let best = g.search(SearchOpts { max_depth: 6, noise: 0, time_ms: 20_000.0 }).expect("a move");
        assert_eq!(strip(&best.san), p.line[0], "{}: the engine prefers {} (score {})", p.id, best.san, best.score);
        let before = material(&g);
        for san in p.line {
            let legal = g.legal();
            let m = legal.iter().copied().find(|&m| strip(&g.san(m, Some(&legal))) == *san);
            g.play(m.unwrap_or_else(|| panic!("{}: {san} is not legal", p.id)));
        }
        assert!(material(&g) >= before + 2, "{}: the line wins too little ({} to {})", p.id, before, material(&g));
    }
}

/// Material balance in pawns, from the point of view of whoever was to move at the start.
fn material(g: &Engine) -> i32 {
    const VALUE: [i32; 7] = [0, 1, 3, 3, 5, 9, 0];
    let white: i32 = g.board().iter().map(|&p| VALUE[(p & 7) as usize] * if p >> 3 == 0 { 1 } else { -1 }).sum();
    let solver_is_white = (g.turn() == 0) == (g.history().len() % 2 == 0);
    if solver_is_white {
        white
    } else {
        -white
    }
}
