//! Playing a friend in another browser. One player creates a game and sends the
//! link; the server (server/src/games.rs) holds the position, checks the moves and
//! runs the clocks. The page plays its own moves at once and asks the server, with a
//! request that stays open until something changes, for the other player's.

use super::*;
use web_sys::{RequestInit, Response};

pub struct Online {
    pub id: String,
    /// The secret that proves which player this browser is. Empty when only watching.
    pub seat: String,
    /// 0 White, 1 Black, 2 watching.
    pub me: usize,
    /// The newest version of the game seen from the server.
    version: f64,
    pub joined: bool,
    /// Changes when the game is left, so late replies meant for it are dropped.
    gen: u32,
    /// Who has a draw offer on the table.
    draw_from: Option<usize>,
    /// A move of ours that the server has not confirmed yet.
    pending: bool,
    /// Whether the local game was against the computer, to go back to afterwards.
    pub was_cpu: bool,
    flag_asked: f64,
    offline: bool,
}

/// (HTTP status, or 0 if the server could not be reached; words for the player).
type Failure = (u16, String);

fn field(v: &JsValue, key: &str) -> JsValue {
    js_sys::Reflect::get(v, &JsValue::from_str(key)).unwrap_or(JsValue::UNDEFINED)
}

async fn http(post: bool, url: String, body: String) -> Result<JsValue, Failure> {
    let unreachable = || (0u16, String::from("Could not reach the server."));
    let init = RequestInit::new();
    if post {
        init.set_method("POST");
        init.set_body(&JsValue::from_str(&body));
    }
    let resp: Response = JsFuture::from(window().fetch_with_str_and_init(&url, &init))
        .await
        .map_err(|_| unreachable())?
        .dyn_into()
        .map_err(|_| unreachable())?;
    let text = JsFuture::from(resp.text().map_err(|_| unreachable())?).await.ok().and_then(|t| t.as_string());
    let json = text.and_then(|t| js_sys::JSON::parse(&t).ok()).filter(|v| v.is_object());
    match json {
        Some(v) if resp.ok() => Ok(v),
        Some(v) => Err((resp.status(), field(&v, "error").as_string().unwrap_or_else(|| "The server refused that.".into()))),
        // A host that only serves files answers with a page, not with a game.
        None => Err((
            resp.status(),
            String::from("Online play needs the Sixty-Four server. This copy of the site is served as plain files."),
        )),
    }
}

impl App {
    /// Create a game on the server and show the link to send.
    pub fn online_invite(&mut self) {
        let color = el("on-color").unchecked_into::<HtmlSelectElement>().value();
        let (base, inc) = TIME_CONTROLS[self.play.tc];
        set_text("online-msg", "Creating the game…");
        spawn_local(async move {
            let made = http(true, "api/new".into(), format!("color={color}&base={base}&inc={inc}")).await;
            with_app(|a| match made {
                Ok(v) => a.online_begin(
                    field(&v, "id").as_string().unwrap_or_default(),
                    field(&v, "seat").as_string().unwrap_or_default(),
                    field(&v, "color").as_f64().unwrap_or(0.0) as usize,
                ),
                Err((_, why)) => set_text("online-msg", &why),
            });
        });
    }

    /// The page was opened from an invitation: take the free seat in game `id`.
    pub fn online_join(&mut self, id: String) {
        set_text("online-msg", "Joining the game…");
        spawn_local(async move {
            let joined = http(true, "api/join".into(), format!("id={id}")).await;
            with_app(|a| match joined {
                Ok(v) => a.online_begin(
                    id,
                    field(&v, "seat").as_string().unwrap_or_default(),
                    field(&v, "color").as_f64().unwrap_or(2.0) as usize,
                ),
                Err((_, why)) => set_text("online-msg", &why),
            });
        });
    }

    /// Switch the Play board over to online game `id` and start listening to the server.
    pub fn online_begin(&mut self, id: String, seat: String, me: usize) {
        self.online_leave();
        let was_cpu = self.play.cpu;
        self.online_attach(id, seat, me, was_cpu);
        verdict("play-verdict", "", "Online", "Waiting for your friend to open the link.");
    }

    /// Also used when the page is reloaded in the middle of an online game.
    pub fn online_attach(&mut self, id: String, seat: String, me: usize, was_cpu: bool) {
        self.online_gen += 1;
        self.play.cpu = false;
        self.play.online = Some(Online {
            id,
            seat,
            me: me.min(2),
            version: 0.0,
            joined: false,
            gen: self.online_gen,
            draw_from: None,
            pending: false,
            was_cpu,
            flag_asked: 0.0,
            offline: false,
        });
        self.play_new_game(None);
        // The server says whether there is a clock, and what it reads.
        self.play.clock = None;
        set_text("online-msg", "");
        self.play_refresh();
        self.online_show();
        self.online_poll();
    }

    /// Stop playing online. Walking out of a game that is under way resigns it.
    pub fn online_leave(&mut self) {
        let Some(o) = self.play.online.take() else { return };
        self.online_gen += 1;
        self.play.cpu = o.was_cpu;
        self.play.clock = None;
        if matches!(self.play.confirm, Some(Confirm::AnswerDraw)) {
            self.play.confirm = None;
            set_hidden("play-confirm", true);
        }
        let live = o.me < 2 && o.joined && !self.play.g.history().is_empty() && !self.play.status().over;
        if live {
            spawn_local(async move {
                let _ = http(true, "api/act".into(), format!("id={}&seat={}&what=resign", o.id, o.seat)).await;
            });
        }
        self.online_show();
        self.play_sync_controls();
    }

    /// The address a friend opens to join, or a spectator to watch.
    fn online_link(&self) -> String {
        let (Some(o), loc) = (&self.play.online, window().location()) else { return String::new() };
        format!("{}{}?g={}", loc.origin().unwrap_or_default(), loc.pathname().unwrap_or_default(), o.id)
    }

    /// Bring the "Play a friend online" panel up to date.
    pub fn online_show(&self) {
        let o = self.play.online.as_ref();
        set_hidden("online-idle", o.is_some());
        set_hidden("online-live", o.is_none());
        let Some(o) = o else { return };
        el("invite-link").unchecked_into::<HtmlInputElement>().set_value(&self.online_link());
        let text = if o.offline {
            "Connection lost. Trying again…"
        } else if o.me == 2 {
            "You are watching this game."
        } else if !o.joined {
            "Send this link to a friend. The game starts when they open it."
        } else if o.me == 0 {
            "You are White, playing a friend online."
        } else {
            "You are Black, playing a friend online."
        };
        set_text("online-state", text);
    }

    pub fn online_copy_link(&mut self) {
        let link = self.online_link();
        el("invite-link").unchecked_into::<HtmlInputElement>().select();
        let fallback = "Press Ctrl+C to copy the link.";
        match clipboard_write(&link) {
            Some(done) => spawn_local(async move {
                let copied = JsFuture::from(done).await.is_ok();
                set_text("link-msg", if copied { "Link copied." } else { fallback });
            }),
            None => set_text("link-msg", fallback),
        }
    }

    // ---- talking to the server ----

    /// Ask for the game, and keep asking: each request is answered when something changes.
    fn online_poll(&mut self) {
        let Some(o) = &self.play.online else { return };
        let gen = o.gen;
        let url = format!("api/state?id={}&seat={}&v={}", o.id, o.seat, o.version);
        spawn_local(async move {
            let reply = http(false, url, String::new()).await;
            with_app(|a| a.online_polled(gen, reply));
        });
    }

    fn online_polled(&mut self, gen: u32, reply: Result<JsValue, Failure>) {
        let Some(o) = &mut self.play.online else { return };
        if o.gen != gen {
            return;
        }
        match reply {
            Ok(v) => {
                o.offline = false;
                self.online_state(&v);
                self.online_poll();
            }
            Err((status, _)) if status == 0 || status >= 500 => {
                o.offline = true;
                self.online_show();
                set_timeout(2500, move || {
                    with_app(|a| {
                        if a.play.online.as_ref().is_some_and(|o| o.gen == gen) {
                            a.online_poll();
                        }
                    })
                });
            }
            Err((_, why)) => {
                // The game is gone (the server restarted, or it was left idle too long).
                if let Some(o) = self.play.online.take() {
                    self.play.cpu = o.was_cpu;
                }
                self.online_gen += 1;
                self.online_show();
                self.play_sync_controls();
                self.play_new_game(None);
                set_text("online-msg", &why);
            }
        }
    }

    /// Send a request about this game; the answer is the game as it now stands.
    fn online_post(&mut self, path: &'static str, extra: String, is_move: bool) {
        let Some(o) = &self.play.online else { return };
        let gen = o.gen;
        let body = format!("id={}&seat={}{extra}", o.id, o.seat);
        let state_url = format!("api/state?id={}&seat={}&v=0", o.id, o.seat);
        spawn_local(async move {
            let mut reply = http(true, format!("api/{path}"), body).await;
            let refused = reply.as_ref().err().map(|(_, why)| why.clone());
            if refused.is_some() {
                // Find out where the game really stands.
                reply = http(false, state_url, String::new()).await;
            }
            with_app(|a| {
                let Some(o) = &mut a.play.online else { return };
                if o.gen != gen {
                    return;
                }
                if is_move {
                    o.pending = false;
                }
                if let Some(why) = refused {
                    verdict("play-verdict", "no", "Online", &why);
                }
                if let Ok(v) = reply {
                    a.online_state(&v);
                }
            });
        });
    }

    /// Our own move has been played on the board; tell the server.
    pub fn online_send_move(&mut self, san: String, ply: usize) {
        if let Some(o) = &mut self.play.online {
            o.pending = true;
        }
        let san: String = js_sys::encode_uri_component(&san).into();
        self.online_post("move", format!("&san={san}&ply={ply}"), true);
    }

    /// `what` is "resign", "offer", "accept" or "decline".
    pub fn online_act(&mut self, what: &'static str) {
        self.online_post("act", format!("&what={what}"), false);
    }

    /// A clock on this screen has reached zero: have the server check its own.
    pub fn online_claim_flag(&mut self) {
        let Some(o) = &mut self.play.online else { return };
        if now() - o.flag_asked < 1500.0 {
            return;
        }
        o.flag_asked = now();
        self.online_post("act", String::from("&what=flag"), false);
    }

    /// How an online game ended other than on the board, in words and as a score.
    fn online_end(&self, end: &str) -> Option<(String, &'static str)> {
        let (kind, side) = end.split_once(':').unwrap_or((end, ""));
        let loser = usize::from(side == "1");
        let lost = if loser == 0 { "0-1" } else { "1-0" };
        match kind {
            "draw" => Some((String::from("Draw agreed."), "1/2-1/2")),
            "resign" => Some((self.play_loss_text(loser, "resigned"), lost)),
            "time" => Some(self.play_time_result(loser, true)),
            "timedraw" => Some(self.play_time_result(loser, false)),
            _ => None,
        }
    }

    /// The server's account of the game has arrived: make the board agree with it.
    fn online_state(&mut self, v: &JsValue) {
        let Some(o) = &mut self.play.online else { return };
        let version = field(v, "v").as_f64().unwrap_or(0.0);
        if version < o.version {
            return;
        }
        o.version = version;
        let joined = field(v, "joined").as_bool().unwrap_or(false);
        let newly_joined = joined && !o.joined;
        o.joined = joined;
        o.me = (field(v, "you").as_f64().unwrap_or(2.0) as usize).min(2);
        let offer = field(v, "draw").as_f64().filter(|d| *d >= 0.0).map(|d| d as usize);
        let old_offer = std::mem::replace(&mut o.draw_from, offer);
        let (me, pending) = (o.me, o.pending);
        let moves: Vec<String> =
            field(v, "moves").as_string().unwrap_or_default().split_whitespace().map(String::from).collect();
        let end = field(v, "end").as_string().unwrap_or_default();

        // Moves: play what is new; if the two accounts disagree, the server's stands.
        let g = &mut self.play.g;
        let mine: Vec<String> = g.history().iter().map(|h| h.san.clone()).collect();
        let mut arrived = None;
        if moves.len() >= mine.len() && moves[..mine.len()] == mine[..] {
            for san in &moves[mine.len()..] {
                let mover = g.turn();
                arrived = g.play_san(san).map(|m| (m, mover, san.clone()));
            }
            if moves.len() > mine.len() + 1 {
                // Catching up after a reload: show the position, not a slide.
                arrived = None;
                self.boards[PLAY].clear();
            }
        } else if pending && mine.len() == moves.len() + 1 && mine[..moves.len()] == moves[..] {
            // Our latest move is still on its way to the server.
        } else {
            g.load(START);
            for san in &moves {
                g.play_san(san);
            }
            self.boards[PLAY].clear();
            self.play.premove = None;
        }

        let clock = field(v, "clock");
        self.play.clock = js_sys::Array::is_array(&clock).then(|| {
            let a = js_sys::Array::from(&clock);
            let running = field(v, "running").as_bool().unwrap_or(false);
            Clock {
                left: [a.get(0).as_f64().unwrap_or(0.0), a.get(1).as_f64().unwrap_or(0.0)],
                base: 0.0,
                inc: field(v, "inc").as_f64().unwrap_or(0.0),
                since: running.then(now),
                log: Vec::new(),
            }
        });

        if self.play.ended.is_none() {
            if let Some((text, result)) = self.online_end(&end) {
                self.play.ended = Some(Ended { text, result });
                self.play.premove = None;
                if end.starts_with("resign") {
                    self.sound.play(Sfx::Topple, 0.0);
                }
                let how = if end == "draw" { "The game is drawn." } else { "The game is over." };
                verdict("play-verdict", "", "Online", &format!("{how} Go back through the moves to look it over."));
            }
        }

        let asked = matches!(self.play.confirm, Some(Confirm::AnswerDraw));
        if asked && (offer.is_none() || self.play.ended.is_some()) {
            self.play.confirm = None;
            set_hidden("play-confirm", true);
        }
        if offer != old_offer && self.play.ended.is_none() && me < 2 {
            match offer {
                Some(from) if from != me => {
                    self.play_ask(Confirm::AnswerDraw, "Your friend offers a draw.", "Accept draw", "Decline");
                }
                None if old_offer == Some(me) => {
                    verdict("play-verdict", "no", "Draw", "Your draw offer was not taken up.");
                }
                _ => {}
            }
        }

        if newly_joined {
            self.sound.play(Sfx::Setup, 0.0);
            let text = match me {
                0 => "Your friend has joined. You are White, so the first move is yours.",
                1 => "You are in. You are Black; your friend moves first.",
                _ => "Both players are here.",
            };
            verdict("play-verdict", "ok", "Online", text);
        }
        if let Some((m, mover, san)) = &arrived {
            self.play.view = None;
            self.play.hint = None;
            self.boards[PLAY].deselect();
            let sfx = sfx_for(&mut self.play.g, *m);
            self.show_move(PLAY, *m, sfx);
            self.play_announce(*mover, san);
        }
        self.play_refresh();
        self.play_prime();
        self.online_show();
        if arrived.is_some() {
            self.play_run_premove();
        }
    }
}
