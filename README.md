# Sixty-Four Chess (Rust)

A chess site with four sections: play against the computer (with a move coach and
evaluation bar) or a friend online, lessons, an openings explorer (19 openings) and
puzzles (30, from mate in one to mate in three). Everything is Rust; the front end is
compiled to WebAssembly.

## Playing

- **The game is kept.** Whatever is on the Play board is saved in the browser after every
  move, so closing the tab or refreshing brings back the same game, settings included.
- **Going back through the moves.** Click any move in the list, use the buttons under it, or
  press ←, →, Home and End. The evaluation bar follows the position shown. Moves the coach
  disliked are marked `?!`, `?` or `??`, and a finished game shows a count of each.
- **Resign and draw.** The computer takes a draw when it is clearly worse, or after move 30
  when it has no real advantage. With two players, a draw is by agreement. Undo after
  either puts the game back on.
- **Abandoning a game asks first.** New game, changing colour, starting from an opening and
  loading something all ask before throwing away an unfinished game.
- **Save and load.** Copy or download the game as PGN, copy the position as FEN, or paste
  either into the box and press Load. A pasted game opens in two-player mode at its first
  move so it can be played through; a pasted position starts a new game from there.

- **Clocks.** Choose a time control under Game setup: 1, 3+2, 5, 10, 15+10 or 30 minutes.
  The clocks start with the first move. Running out of time loses, or draws if the other
  side has nothing left to mate with. Undo gives the time back.
- **Moves in advance.** While the other side is thinking, make your next move as usual. It
  is marked on the board and played the moment it becomes legal. Clicking anywhere else
  calls it off.
- **Arrows and circles.** Drag with the right mouse button to draw an arrow, right-click a
  square to circle it. Doing the same again removes it; a left click clears them all.
- **Keyboard.** Tab to the board, move the cursor with the arrow keys, and press Enter or
  Space to pick a piece up and put it down. Escape cancels. Or type a move in the box under
  the board: `Nf3`, `exd5`, `O-O`, or just the squares, `e2e4`. Moves are announced for
  screen readers.
- **Settings.** The Settings button changes the board colours, the piece set, how fast the
  pieces move, and the volume. The choices are remembered.
- **Offline.** After the first visit the site opens without a connection, and it can be
  installed as an app from the browser's menu. Online games still need the network.

## Playing a friend online

Under "Play a friend online", pick a colour and press **Create invite link**, then send the
link. The game starts when your friend opens it. It uses the clock chosen under Game setup,
and anyone else who opens the link watches. Hints, undo and the coach are off during the
game. Refreshing the page returns to the same game.

This is the one part that needs the Rust server rather than plain file hosting. The server
checks every move with the engine and keeps the clocks, so neither browser can cheat the
other. Games live in memory: they are forgotten after six hours without a move, and when
the server restarts.

| Request | Form fields | What it does |
| --- | --- | --- |
| `POST api/new` | `color` (`w`, `b`, `r`), `base`, `inc` (seconds) | Creates a game; replies with its `id` and your `seat` |
| `POST api/join` | `id` | Takes the empty seat, or watches if both are taken |
| `GET api/state` | `id`, `seat`, `v` | The game; waits up to 25 s for a change after version `v` |
| `POST api/move` | `id`, `seat`, `san`, `ply` | Plays a move |
| `POST api/act` | `id`, `seat`, `what` | `resign`, `offer`, `accept`, `decline` or `flag` |

## Layout

| Path | What it is |
| --- | --- |
| `engine/` | Chess rules, SAN, FEN, PGN and alpha-beta search. No dependencies; tested natively. |
| `web/` | The UI (`wasm-bindgen` + `web-sys`). `lib.rs` holds the board and the four sections; `clock.rs`, `keys.rs`, `online.rs`, `settings.rs` and `sound.rs` are what their names say; `content.rs` is the lessons, openings and puzzles. |
| `server/` | Serves the built site and hosts online games (`games.rs`). Standard library only, plus the engine. |
| `static/` | `index.html` (markup and CSS), `worker.js` (loads the engine in a Web Worker), `sw.js` (offline support), `manifest.webmanifest` and icons, `pieces/` (SVG piece images) and `sounds/` (recorded piece sounds). |
| `dist/` | Build output. |
| `Dockerfile`, `.github/workflows/pages.yml` | The two ways to put it on the internet, below. |

## Build and run

Requirements: Rust with the `wasm32-unknown-unknown` target, and `wasm-bindgen-cli`
at the same version as the `wasm-bindgen` crate in `Cargo.lock` (currently 0.2.130).

```powershell
.\build.ps1                                  # compiles to dist/   (sh build.sh elsewhere)
cargo run --release -p sixtyfour-server      # http://localhost:8080
```

The server takes an optional port and directory: `serve 3000 dist`. It listens on this
computer only unless the `HOST` environment variable says otherwise (`HOST=0.0.0.0` for
everyone on the network); `PORT` sets the port. The page must be served over HTTP;
browsers will not load WebAssembly from a `file://` URL.

## Putting it on the internet

There are two ways, depending on whether online play matters.

**With online play: run the container.** The `Dockerfile` builds the site and the server
into one image. Any host that runs a Dockerfile will do (Fly.io, Render, Railway, Cloud
Run, a VPS):

```sh
docker build -t sixtyfour .
docker run -p 8080:8080 sixtyfour
```

Run a single instance: games are held in that one process's memory.

**Without online play: any static host.** `dist/` is a complete site. Upload it to
GitHub Pages, Netlify, Cloudflare Pages or similar. For GitHub Pages the included workflow
does it on every push to `main`: push this folder to a GitHub repository and choose
Settings > Pages > Source: "GitHub Actions". On a static host the "Create invite link"
button explains that online play needs the server.

Both pin `wasm-bindgen-cli` to 0.2.130; change it there if the crate is updated.

## Tests

```powershell
cargo test -p sixtyfour-engine --release
cargo test -p sixtyfour-server --release
```

The engine tests check move generation against known perft counts, plus FEN reading and
validation, SAN, PGN reading and writing, and mate search. They also check the site's
content: every lesson position and answer is legal, every opening line plays through,
every "mate in N" puzzle is mate in exactly N, and every tactics puzzle's first move is
the one the engine picks. The server tests play online games through: joining, moves out
of turn, illegal moves, draw offers, resignation and running out of time.

## Credits

The piece images, all as distributed with lichess.org:

| Folder | Set | Author | Licence |
| --- | --- | --- | --- |
| `static/pieces/` | cburnett | Colin M.L. Burnett | GPLv2+ / BSD / GFDL |
| `static/pieces/merida/` | Merida | Armando Hernandez Marroquin | GPLv2+ |
| `static/pieces/chessnut/` | Chessnut | Alexis Luengas | Apache 2.0 |

The sounds in `static/sounds/` are short cuts from recordings of real chess pieces on
real boards, all published on Freesound under CC0 (public domain). `web/src/sound.rs`
layers and varies them; nothing is synthesised.

| Files | What it is | Source |
| --- | --- | --- |
| `move1`–`move5` | A piece set down on a wooden board | [Chess Pieces Move (Close)](https://freesound.org/s/733927/) by JJTaynos |
| `clack1` | One piece knocking another | [Chess Pieces Drop](https://freesound.org/s/755250/) by IENBA |
| `clack2`, `clack3` | One piece knocking another | [D9_pawn](https://freesound.org/s/371352/) by Iamgiorgio |
| `thump` | A piece put down hard (check, mate) | [Chess_foley](https://freesound.org/s/629181/) by Amatsuuu |
| `tick1`, `tick2` | A piece touched or picked up | [Chess Pieces](https://freesound.org/s/765866/) by MattyCauldron |
| `topple` | A king falling over (mate, resignation) | [Chess Pieces Drop](https://freesound.org/s/755250/) by IENBA |
| `slide` | A piece slid back after a wrong try | [dragslide1](https://freesound.org/s/396705/) by ChaosEntertainment |
| `setup` | Pieces being set up (new game) | [Board Start](https://freesound.org/s/546121/) by el_boss |

Each cut was trimmed, high-passed, lightly saturated and normalised.
