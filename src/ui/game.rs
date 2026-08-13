//! agtop — easter egg side-scrolling dodger.
//!
//! Toggled with `` ` `` (backtick).  Replaces the bottom-right
//! sessions panel.  Player ship autopilots; user verbs are SPACE
//! (fire laser, 10 energy) and `b` (bomb, 60 energy).  Energy is
//! refilled from the live snapshot's tokens_rate, so the game can
//! only "do things" when the monitored agents are doing things.
//!
//! Frame rate is naturally capped at ~10 fps by the parent event
//! loop (`event::poll` timeout = 100 ms).  Per-frame work is a
//! few-dozen-entity physics step + one ratatui draw.  A guard in
//! `tick()` self-disables the game if it overshoots 5 ms wall-time
//! three frames in a row — agtop's purpose is monitoring, not
//! gaming.

use crate::model::Snapshot;
use crate::theme;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};
use std::time::Instant;

// Fallback field dimensions used until the first draw sets the
// real ones.  The playfield otherwise tracks the panel's inner
// Rect each frame (see `field_w` / `field_h` on `GameState`),
// so the game adapts to panel-mode vs full-screen, terminal
// resizes, and theme padding without any baked-in assumptions.
const FALLBACK_FIELD_W: i32 = 60;
const FALLBACK_FIELD_H: i32 = 14;

const SHIP_X: i32 = 4;

// Energy economy.
const ENERGY_MAX: f32 = 100.0;
const LASER_COST: f32 = 10.0;
const BOMB_COST:  f32 = 60.0;
// Baseline regen so the player isn't stuck when agents are quiet.
const ENERGY_BASELINE_PER_SEC: f32 = 4.0;
// 1 energy per N tokens flowing through the monitored agents.
const TOKENS_PER_ENERGY: f32 = 300.0;
// Hard cap on token-driven refill — no infinite-energy bursts.
const ENERGY_REFILL_MAX_PER_SEC: f32 = 40.0;

// Snapshot → game-parameter mapping.
const SCROLL_MIN: f32 = 4.0;   // cells/sec at 0% aggregate CPU
const SCROLL_MAX: f32 = 36.0;  // cells/sec at saturated host
const SPAWN_MIN:  f32 = 1.0;   // obstacles/sec when nothing is busy
const SPAWN_MAX:  f32 = 6.0;   // obstacles/sec when host is hot
// Boss appears each time aggregate cost crosses this many USD.
const BOSS_COST_STEP: f64 = 0.10;

// Budget guard.
const FRAME_BUDGET_US: u128 = 5_000;
const SLOW_FRAME_TRIP: u8 = 3;

pub(super) struct GameState {
    /// Current playfield width in cells.  Updated by `draw` each
    /// frame from the panel's inner Rect; `tick` reads it for spawn
    /// X / scroll-cull boundaries.  Starts at the fallback constant
    /// so the very first tick (before any draw) doesn't operate on
    /// a zero-size world.
    field_w: i32,
    /// Current playfield height (excludes the HUD row).  Same
    /// lifecycle as `field_w`.
    field_h: i32,
    ship_y: i32,
    ship_target_y: i32,
    obstacles: Vec<Obstacle>,
    lasers: Vec<Laser>,
    particles: Vec<Particle>,
    energy: f32,
    score: u64,
    high_score: u64,
    lives: u8,
    last_frame: Instant,
    /// Monotonically increasing frame counter — drives the idle
    /// sinusoidal wander when no threats are in lookahead.
    frame: u64,
    rng: u64,
    scroll_accum: f32,
    boss: Option<Boss>,
    prev_cost: f64,
    // Smoothed copies of the snapshot-derived parameters so values
    // don't strobe on each new snapshot.
    cur_scroll: f32,
    cur_spawn:  f32,
    cur_refill: f32,
    // Self-protection.
    slow_frames: u8,
    pub(super) disabled_self: bool,
    // Game-over latch.  Any key resets when set.
    over: bool,
    /// `true` when the game has been promoted to occupy the whole
    /// body area (replacing every panel between header and footer).
    /// Toggled with `Z`; `Esc` demotes back to panel mode.
    pub(super) fullscreen: bool,
}

#[derive(Clone, Copy)]
struct Obstacle {
    x: f32,
    y: i32,
    kind: ObstacleKind,
    hp: i32,
}

#[derive(Clone, Copy)]
enum ObstacleKind { Small, Large, Danger }

#[derive(Clone, Copy)]
struct Laser {
    x: f32,
    /// X of this laser at the start of the current frame.  Used for
    /// swept collision so a fast-moving laser (~6 cells/frame) can't
    /// tunnel through a 1-cell obstacle between ticks.
    prev_x: f32,
    y: i32,
}

#[derive(Clone, Copy)]
struct Particle { x: i32, y: i32, ttl: u8, ch: char, color: Color }

#[derive(Clone, Copy)]
struct Boss {
    x: f32,
    y: f32,
    hp: i32,
    dy: f32,  // cells/sec, sign flips at edges
}

pub(super) enum KeyDispatch {
    Handled,
    CloseGame,
}

impl GameState {
    pub(super) fn new() -> Self {
        let now = Instant::now();
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0xC0FFEE_FEEDFACE);
        Self {
            field_w: FALLBACK_FIELD_W,
            field_h: FALLBACK_FIELD_H,
            ship_y: FALLBACK_FIELD_H / 2,
            ship_target_y: FALLBACK_FIELD_H / 2,
            obstacles: Vec::with_capacity(32),
            lasers: Vec::with_capacity(8),
            particles: Vec::with_capacity(32),
            energy: ENERGY_MAX * 0.5,
            score: 0,
            high_score: 0,
            lives: 3,
            last_frame: now,
            frame: 0,
            // LCG demands an odd seed for full period; OR in 1 to be safe.
            rng: seed | 1,
            scroll_accum: 0.0,
            boss: None,
            prev_cost: 0.0,
            cur_scroll: SCROLL_MIN,
            cur_spawn:  SPAWN_MIN,
            cur_refill: ENERGY_BASELINE_PER_SEC,
            slow_frames: 0,
            disabled_self: false,
            over: false,
            fullscreen: false,
        }
    }

    fn reset(&mut self) {
        self.obstacles.clear();
        self.lasers.clear();
        self.particles.clear();
        self.energy = ENERGY_MAX * 0.5;
        self.score = 0;
        self.lives = 3;
        // Keep field_w / field_h as last seen — draw will refresh
        // them next frame; this avoids a one-frame snap to fallback.
        self.ship_y = self.field_h / 2;
        self.ship_target_y = self.field_h / 2;
        self.boss = None;
        self.scroll_accum = 0.0;
        self.over = false;
        // last_frame is reset so the first post-restart dt isn't huge.
        self.last_frame = Instant::now();
    }

    // PCG-style step on a 64-bit LCG; high bits are the output.
    fn rand(&mut self) -> u32 {
        self.rng = self.rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.rng >> 33) as u32
    }

    fn rand_range(&mut self, lo: i32, hi_exclusive: i32) -> i32 {
        if hi_exclusive <= lo { return lo; }
        let span = (hi_exclusive - lo) as u32;
        lo + (self.rand() % span) as i32
    }

    fn chance(&mut self, p: f32) -> bool {
        let p = p.clamp(0.0, 1.0);
        (self.rand() as f32 / u32::MAX as f32) < p
    }

    pub(super) fn handle_key(&mut self, key: KeyEvent) -> KeyDispatch {
        if key.kind != KeyEventKind::Press { return KeyDispatch::Handled; }
        // Game-over: any non-exit key resets.
        if self.over {
            return match key.code {
                KeyCode::Char('`') | KeyCode::Esc => KeyDispatch::CloseGame,
                _ => { self.reset(); KeyDispatch::Handled }
            };
        }
        match key.code {
            // Backtick: always closes the game outright, regardless of
            // panel/fullscreen mode.  Z: toggle fullscreen.  Esc: back
            // out one level — fullscreen → panel, then panel → close.
            KeyCode::Char('`') => KeyDispatch::CloseGame,
            KeyCode::Esc => {
                if self.fullscreen {
                    self.fullscreen = false;
                    KeyDispatch::Handled
                } else {
                    KeyDispatch::CloseGame
                }
            }
            KeyCode::Char('Z') => {
                self.fullscreen = !self.fullscreen;
                KeyDispatch::Handled
            }
            KeyCode::Char(' ') => {
                if self.energy >= LASER_COST && !self.disabled_self {
                    self.energy -= LASER_COST;
                    let lx0 = (SHIP_X + 2) as f32;
                    self.lasers.push(Laser {
                        x: lx0, prev_x: lx0, y: self.ship_y,
                    });
                }
                KeyDispatch::Handled
            }
            KeyCode::Char('b') => {
                if self.energy >= BOMB_COST && !self.disabled_self {
                    self.energy -= BOMB_COST;
                    // Bomb kills every on-screen obstacle and dents boss.
                    let kills = self.obstacles.len() as u64;
                    self.score = self.score.saturating_add(kills * 5);
                    for o in &self.obstacles {
                        self.particles.push(Particle {
                            x: o.x as i32, y: o.y, ttl: 6,
                            ch: '✺', color: theme::c_wait(),
                        });
                    }
                    self.obstacles.clear();
                    if let Some(b) = self.boss.as_mut() {
                        b.hp -= 20;
                    }
                }
                KeyDispatch::Handled
            }
            _ => KeyDispatch::Handled,
        }
    }

    pub(super) fn tick(&mut self, snap: &Snapshot) {
        if self.disabled_self || self.over { return; }
        // Resize defence: panel↔fullscreen toggles or a terminal
        // resize can shrink the field under entities that were
        // valid last frame.  Clamp aggressively so nothing's
        // permanently off-grid.
        let max_y = (self.field_h - 1).max(0);
        let max_x = (self.field_w - 1).max(0) as f32;
        self.ship_y = self.ship_y.clamp(0, max_y);
        self.ship_target_y = self.ship_target_y.clamp(0, max_y);
        for o in &mut self.obstacles {
            if o.y > max_y { o.y = max_y; }
            if o.y < 0    { o.y = 0; }
            if o.x > max_x { o.x = max_x; }
        }
        for l in &mut self.lasers {
            if l.y > max_y { l.y = max_y; }
        }
        if let Some(b) = self.boss.as_mut() {
            if b.x > max_x { b.x = max_x; }
            let b_max_y = (self.field_h - 2).max(1) as f32;
            if b.y > b_max_y { b.y = b_max_y; }
            if b.y < 1.0 { b.y = 1.0; }
        }
        let frame_start = Instant::now();
        let dt_s = frame_start
            .saturating_duration_since(self.last_frame)
            .as_micros() as f32 / 1_000_000.0;
        let dt_s = dt_s.clamp(0.0, 0.5);  // cap dt so a paused terminal doesn't catapult state
        self.last_frame = frame_start;

        // ── snapshot → game parameter mapping ─────────────────────
        let a = &snap.aggregates;
        let cpu_norm = (a.cpu as f32 / (snap.sys_cpus.max(1) as f32 * 100.0))
            .clamp(0.0, 1.0);
        let busy_load = (a.busy + a.subagents) as f32;
        // Average the last 8 token-rate samples (each = tokens per
        // snapshot tick).  We divide by an assumed ~1.5s interval to
        // get per-second; this is approximate but the energy economy
        // tolerates it.
        let tokens_per_sec = if !snap.history.tokens_rate.is_empty() {
            let take = snap.history.tokens_rate.len().min(8);
            let sum: f64 = snap.history.tokens_rate.iter().rev().take(take).sum();
            (sum as f32 / take as f32) / 1.5
        } else { 0.0 };
        let target_scroll = lerp(SCROLL_MIN, SCROLL_MAX, cpu_norm);
        let target_spawn  = (SPAWN_MIN + busy_load * 0.5)
            .clamp(SPAWN_MIN, SPAWN_MAX);
        let target_refill = (ENERGY_BASELINE_PER_SEC + tokens_per_sec / TOKENS_PER_ENERGY)
            .min(ENERGY_REFILL_MAX_PER_SEC);
        // Exponential smoothing — ~3 ticks to 95%.
        let alpha = 0.3;
        self.cur_scroll = self.cur_scroll + (target_scroll - self.cur_scroll) * alpha;
        self.cur_spawn  = self.cur_spawn  + (target_spawn  - self.cur_spawn)  * alpha;
        self.cur_refill = self.cur_refill + (target_refill - self.cur_refill) * alpha;

        // Boss spawn: aggregate cost crossing the next $BOSS_COST_STEP step.
        if self.boss.is_none() {
            let prev_step = (self.prev_cost / BOSS_COST_STEP).floor() as i64;
            let cur_step  = (a.cost_usd     / BOSS_COST_STEP).floor() as i64;
            if cur_step > prev_step && a.cost_usd > 0.0 {
                self.boss = Some(Boss {
                    x: (self.field_w - 4) as f32,
                    y: (self.field_h / 2) as f32,
                    hp: 40,
                    dy: 6.0,
                });
            }
        }
        self.prev_cost = a.cost_usd;

        // ── energy ────────────────────────────────────────────────
        self.energy = (self.energy + self.cur_refill * dt_s).min(ENERGY_MAX);

        // ── scroll obstacles & boss ───────────────────────────────
        self.scroll_accum += self.cur_scroll * dt_s;
        let cells = self.scroll_accum.floor();
        self.scroll_accum -= cells;
        if cells > 0.0 {
            for o in &mut self.obstacles { o.x -= cells; }
            // Boss creeps in then holds station near the right edge.
            // Read field_w into a local first so the &mut self.boss
            // borrow below doesn't conflict with reading self.field_w.
            let fw = self.field_w;
            if let Some(b) = self.boss.as_mut() {
                if b.x > (fw - 10) as f32 { b.x -= cells * 0.5; }
            }
        }
        // Lasers move right at fixed speed.  Stash prev_x first so
        // the collision pass can sweep the segment [prev_x, x] and
        // catch obstacles even when the per-frame jump exceeds the
        // obstacle width (60 cells/sec × 0.1s = 6 cells/frame at 10fps).
        // Laser speed tuned to ~4 cells/frame at 10fps — fast enough
        // to feel like a shot, slow enough that the trail is visible
        // and the swept collision below has reasonable granularity.
        for l in &mut self.lasers {
            l.prev_x = l.x;
            l.x += 40.0 * dt_s;
        }
        // Boss vertical sweep — bounce between row 1 and field_h-2
        // so the boss never overlaps the top border or the HUD row.
        let fh = self.field_h;
        if let Some(b) = self.boss.as_mut() {
            b.y += b.dy * dt_s;
            if b.y < 1.0 { b.y = 1.0; b.dy = b.dy.abs(); }
            let max_y = (fh - 2).max(1) as f32;
            if b.y > max_y { b.y = max_y; b.dy = -b.dy.abs(); }
        }

        // ── cull off-screen, age particles ───────────────────────
        let before = self.obstacles.len();
        self.obstacles.retain(|o| o.x > -2.0);
        // Each obstacle that scrolled off unharmed counts as a dodge.
        self.score = self.score.saturating_add((before - self.obstacles.len()) as u64);
        let fw = self.field_w;
        self.lasers.retain(|l| l.x < (fw + 2) as f32);
        for p in &mut self.particles { p.ttl = p.ttl.saturating_sub(1); }
        self.particles.retain(|p| p.ttl > 0);

        // ── spawn ────────────────────────────────────────────────
        let p_spawn = self.cur_spawn * dt_s;
        if self.chance(p_spawn) {
            let y = self.rand_range(0, self.field_h.max(1));
            // Danger probability mirrors the share of dangerous agents.
            let n_agents = snap.agents.len().max(1);
            let n_danger = snap.agents.iter().filter(|a| a.dangerous).count();
            let danger_p = (n_danger as f32 / n_agents as f32).clamp(0.0, 0.5);
            let kind = if self.chance(danger_p) {
                ObstacleKind::Danger
            } else if self.chance(0.25) {
                ObstacleKind::Large
            } else {
                ObstacleKind::Small
            };
            let hp = match kind {
                ObstacleKind::Small  => 1,
                ObstacleKind::Large  => 2,
                ObstacleKind::Danger => 1,
            };
            self.obstacles.push(Obstacle {
                x: (self.field_w - 1) as f32, y, kind, hp,
            });
        }

        // ── laser/target collisions ──────────────────────────────
        // Borrow-checker dance: capture laser positions first, then
        // mutate obstacles/boss/particles inside the iteration.
        let mut consumed: Vec<usize> = Vec::new();
        let mut new_particles: Vec<Particle> = Vec::new();
        for li in 0..self.lasers.len() {
            let l_lo = self.lasers[li].prev_x.min(self.lasers[li].x);
            let l_hi = self.lasers[li].prev_x.max(self.lasers[li].x);
            let lx_now = self.lasers[li].x.round() as i32;
            let ly = self.lasers[li].y;
            let mut hit_something = false;
            for o in self.obstacles.iter_mut() {
                // Swept hit-test: obstacle's column lies inside the
                // laser's per-frame travel segment (± 0.5 cell each
                // side for rounding tolerance).
                if o.y == ly && o.x >= l_lo - 0.5 && o.x <= l_hi + 0.5 {
                    o.hp -= 1;
                    hit_something = true;
                    if o.hp <= 0 {
                        new_particles.push(Particle {
                            x: o.x.round() as i32, y: o.y, ttl: 4, ch: '✦',
                            color: theme::c_busy(),
                        });
                    }
                    break;
                }
            }
            if let Some(b) = self.boss.as_mut() {
                // Boss is a 3×3 sprite — match against its 3-cell band
                // both vertically and within the laser's swept x-range.
                if (ly - b.y.round() as i32).abs() <= 1
                    && b.x >= l_lo - 1.5 && b.x <= l_hi + 1.5
                {
                    b.hp -= 2;
                    hit_something = true;
                    new_particles.push(Particle {
                        x: lx_now, y: ly, ttl: 3, ch: '*',
                        color: theme::c_wait(),
                    });
                }
            }
            if hit_something { consumed.push(li); }
        }
        self.particles.extend(new_particles);
        // Remove consumed lasers (reverse index order, swap_remove is fine).
        consumed.sort_unstable();
        consumed.dedup();
        for &i in consumed.iter().rev() {
            if i < self.lasers.len() { self.lasers.swap_remove(i); }
        }
        // Tally kills, drop dead obstacles.
        let kills = self.obstacles.iter().filter(|o| o.hp <= 0).count() as u64;
        self.obstacles.retain(|o| o.hp > 0);
        self.score = self.score.saturating_add(kills * 5);
        // Boss kill?
        let mut boss_killed_at: Option<(i32, i32)> = None;
        if let Some(b) = self.boss {
            if b.hp <= 0 {
                boss_killed_at = Some((b.x.round() as i32, b.y.round() as i32));
            }
        }
        if let Some((bx, by)) = boss_killed_at {
            self.score = self.score.saturating_add(50);
            for _ in 0..14 {
                let dx = self.rand_range(-3, 4);
                let dy = self.rand_range(-2, 3);
                self.particles.push(Particle {
                    x: bx + dx, y: by + dy, ttl: 8,
                    ch: '✺', color: theme::c_wait(),
                });
            }
            self.boss = None;
        }

        // ── autopilot: minimise weighted threat over a lookahead ──
        // Threats radiate 1 row up + down so the ship reads "the
        // obstacle three rows up is close" rather than only worrying
        // about its exact row.  Without this, an empty same-row band
        // makes every candidate equal-cost and the ship sits frozen.
        // Lookahead scales with field width — short canvas means we
        // can only see a few columns of obstacles, so don't waste
        // cycles checking the empty far-right.
        let lookahead = (self.field_w - SHIP_X).clamp(4, 20);
        let mut best_row = self.ship_y;
        let mut best_cost = i32::MAX;
        let mut total_threats_seen = 0;
        for cand in 0..self.field_h {
            let mut threat = 0;
            for o in &self.obstacles {
                let dx = o.x.round() as i32 - SHIP_X;
                if dx <= 0 || dx > lookahead { continue; }
                let dy = (o.y - cand).abs();
                if dy > 1 { continue; }
                let weight = match o.kind {
                    ObstacleKind::Danger => 12,
                    ObstacleKind::Large  => 5,
                    ObstacleKind::Small  => 2,
                };
                // Direct hit weighs full; one row off weighs half.
                let row_factor = if dy == 0 { 2 } else { 1 };
                // Closer obstacles weigh more — (lookahead - dx) ∈ [1, lookahead].
                threat += weight * row_factor * (lookahead - dx + 1);
            }
            // Tiny bias toward the current row so the ship doesn't
            // oscillate between equal-cost rows.  Much smaller than
            // any threat so it never overrides a real evasion.
            let bias = (cand - self.ship_y).abs();
            let total = threat + bias;
            if total < best_cost {
                best_cost = total;
                best_row = cand;
            }
            total_threats_seen += threat;
        }
        // Idle wander: no incoming threats at all → trace a slow
        // sinusoid around the centre line so the ship looks alive.
        // Period ≈ 6.3s at 10 fps; amplitude ⅓ of the field.
        self.frame = self.frame.wrapping_add(1);
        if total_threats_seen == 0 {
            let t = self.frame as f32 * 0.1;
            let amp = (self.field_h as f32 / 3.0).max(2.0);
            best_row = (self.field_h as f32 / 2.0 + amp * t.sin()) as i32;
            best_row = best_row.clamp(0, (self.field_h - 1).max(0));
        }
        self.ship_target_y = best_row;
        // Move at most one cell per frame so motion looks deliberate.
        if self.ship_y < self.ship_target_y { self.ship_y += 1; }
        else if self.ship_y > self.ship_target_y { self.ship_y -= 1; }

        // ── ship/obstacle collision ──────────────────────────────
        let mut hit = false;
        self.obstacles.retain(|o| {
            let ox = o.x.round() as i32;
            if ox == SHIP_X && o.y == self.ship_y {
                hit = true;
                false
            } else { true }
        });
        if hit {
            self.lives = self.lives.saturating_sub(1);
            for _ in 0..6 {
                let dx = self.rand_range(-2, 3);
                let dy = self.rand_range(-1, 2);
                self.particles.push(Particle {
                    x: SHIP_X + dx, y: self.ship_y + dy, ttl: 6,
                    ch: '*', color: theme::c_wait(),
                });
            }
            if self.lives == 0 {
                if self.score > self.high_score { self.high_score = self.score; }
                self.over = true;
            }
        }

        // ── budget guard ─────────────────────────────────────────
        let frame_us = frame_start.elapsed().as_micros();
        if frame_us > FRAME_BUDGET_US {
            self.slow_frames = self.slow_frames.saturating_add(1);
            if self.slow_frames >= SLOW_FRAME_TRIP {
                self.disabled_self = true;
            }
        } else {
            self.slow_frames = 0;
        }
    }

    pub(super) fn draw(&mut self, f: &mut Frame, area: Rect) {
        let title = if self.disabled_self {
            " agtop:dodge — disabled (frame budget exceeded) ".to_string()
        } else if self.over {
            format!(" agtop:dodge — GAME OVER · score {} · any key restarts ", self.score)
        } else {
            format!(" agtop:dodge — score {}  high {}  ♥{} ", self.score, self.high_score, self.lives)
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(theme::border()))
            .title(Span::styled(title,
                Style::default().fg(theme::fg()).add_modifier(Modifier::BOLD)));
        let inner = block.inner(area);
        f.render_widget(block, area);

        if self.disabled_self {
            let lines = vec![
                Line::from(Span::styled("  Self-disabled to keep agtop responsive.",
                    Style::default().fg(theme::fg_dim()))),
                Line::from(Span::raw("")),
                Line::from(Span::styled("  Press ` to restore the sessions panel.",
                    Style::default().fg(theme::fg_dim()))),
            ];
            f.render_widget(Paragraph::new(lines), inner);
            return;
        }
        if inner.width < 30 || inner.height < 6 {
            let p = Paragraph::new(format!(
                "  game needs ≥30×6 cells (have {}×{}).",
                inner.width, inner.height
            )).style(Style::default().fg(theme::fg_dim()));
            f.render_widget(p, inner);
            return;
        }

        let w = inner.width as usize;
        let h = inner.height as usize;
        let play_h = h.saturating_sub(1);  // last row = HUD
        if play_h == 0 { return; }
        // Publish the live playfield dimensions so the next `tick`
        // operates on a world that matches what the user sees.  One
        // frame of latency vs the rect change is fine; the resize-
        // clamp at the top of `tick` handles any orphaned entities.
        self.field_w = w as i32;
        self.field_h = play_h as i32;

        // Build a char grid, then collapse same-style runs into spans.
        let mut grid: Vec<Vec<Cell>> = vec![
            vec![Cell::blank(); w];
            play_h
        ];

        // Static-looking starfield — deterministic by row index so it
        // doesn't shimmer.
        for (y, row) in grid.iter_mut().enumerate().take(play_h) {
            let mut x = ((y as u32).wrapping_mul(2654435761) as usize) % 17;
            while x < w {
                row[x] = Cell { ch: '·', color: theme::fg_dim(), bold: false };
                x += 19;
            }
        }

        // Particles → obstacles → boss → lasers → ship (back to front).
        for p in &self.particles {
            put(&mut grid, p.x, p.y, p.ch, p.color, true, w, play_h);
        }
        for o in &self.obstacles {
            let (ch, color) = match o.kind {
                ObstacleKind::Small  => ('•', theme::fg()),
                ObstacleKind::Large  => ('▓', theme::fg()),
                ObstacleKind::Danger => ('!', theme::c_wait()),
            };
            put(&mut grid, o.x.round() as i32, o.y, ch, color, true, w, play_h);
        }
        if let Some(b) = &self.boss {
            let bx = b.x.round() as i32;
            let by = b.y.round() as i32;
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let ch = if dx == 0 && dy == 0 { '◆' } else { '▒' };
                    put(&mut grid, bx + dx, by + dy, ch, theme::c_done(), true, w, play_h);
                }
            }
        }
        // Each laser draws a streak across its swept path so a
        // multi-cell-per-frame motion reads as a beam, not a flash.
        for l in &self.lasers {
            let from = l.prev_x.round() as i32;
            let to   = l.x.round() as i32;
            let (lo, hi) = if from <= to { (from, to) } else { (to, from) };
            for x in lo..=hi {
                put(&mut grid, x, l.y, '─', theme::c_busy(), true, w, play_h);
            }
        }
        // Ship: 3-cell sprite centred on SHIP_X.
        let sy = self.ship_y;
        put(&mut grid, SHIP_X - 1, sy, '>', theme::c_active(), true, w, play_h);
        put(&mut grid, SHIP_X,     sy, '=', theme::c_active(), true, w, play_h);
        put(&mut grid, SHIP_X + 1, sy, '>', theme::c_active(), true, w, play_h);

        // GAME OVER banner.
        if self.over {
            let mid = play_h / 2;
            let msg = format!("GAME OVER · score {} · press any key", self.score);
            let pad = w.saturating_sub(msg.len()) / 2;
            // Clear that row's grid first.
            if mid < grid.len() {
                for c in &mut grid[mid] { *c = Cell::blank(); }
                for (i, ch) in msg.chars().enumerate() {
                    let x = pad + i;
                    if x < w {
                        grid[mid][x] = Cell { ch, color: theme::c_wait(), bold: true };
                    }
                }
            }
        }

        // Render grid → Paragraph lines (coalescing same-style runs).
        let mut lines: Vec<Line> = Vec::with_capacity(h);
        for row in &grid {
            lines.push(coalesce_row(row));
        }
        lines.push(self.hud_line(w));

        f.render_widget(Paragraph::new(lines), inner);
    }

    fn hud_line(&self, w: usize) -> Line<'static> {
        let bar_w = (w / 3).clamp(8, 32);
        let filled = ((self.energy / ENERGY_MAX) * bar_w as f32).round() as usize;
        let filled = filled.min(bar_w);
        let bar_color = if self.energy >= BOMB_COST { theme::c_busy() }
                        else if self.energy >= LASER_COST { theme::c_active() }
                        else { theme::c_wait() };
        let mut bar = String::with_capacity(bar_w * 3);
        for _ in 0..filled { bar.push('█'); }
        for _ in filled..bar_w { bar.push('░'); }
        Line::from(vec![
            Span::styled(" energy ", Style::default().fg(theme::fg_dim())),
            Span::styled(bar, Style::default().fg(bar_color)),
            Span::styled(format!(" {:>3.0}/{:.0}  ", self.energy, ENERGY_MAX),
                Style::default().fg(theme::fg_dim())),
            Span::styled("SPC", Style::default().fg(theme::fg()).add_modifier(Modifier::BOLD)),
            Span::styled(" fire  ", Style::default().fg(theme::fg_dim())),
            Span::styled("b", Style::default().fg(theme::fg()).add_modifier(Modifier::BOLD)),
            Span::styled(" bomb  ", Style::default().fg(theme::fg_dim())),
            Span::styled("Z", Style::default().fg(theme::fg()).add_modifier(Modifier::BOLD)),
            Span::styled(if self.fullscreen { " window  " } else { " zoom  " },
                Style::default().fg(theme::fg_dim())),
            Span::styled("`", Style::default().fg(theme::fg()).add_modifier(Modifier::BOLD)),
            Span::styled(" exit", Style::default().fg(theme::fg_dim())),
        ])
    }
}

#[derive(Clone, Copy)]
struct Cell {
    ch: char,
    color: Color,
    bold: bool,
}
impl Cell {
    fn blank() -> Self { Self { ch: ' ', color: Color::Reset, bold: false } }
}

#[allow(clippy::too_many_arguments)]
fn put(grid: &mut [Vec<Cell>], x: i32, y: i32, ch: char, color: Color, bold: bool, w: usize, h: usize) {
    if x < 0 || y < 0 { return; }
    let (xu, yu) = (x as usize, y as usize);
    if xu >= w || yu >= h { return; }
    grid[yu][xu] = Cell { ch, color, bold };
}

fn coalesce_row(row: &[Cell]) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    if row.is_empty() { return Line::from(spans); }
    let mut buf = String::new();
    let mut cur = row[0];
    buf.push(cur.ch);
    for c in &row[1..] {
        if c.color == cur.color && c.bold == cur.bold {
            buf.push(c.ch);
        } else {
            spans.push(span_of(std::mem::take(&mut buf), cur));
            buf.push(c.ch);
            cur = *c;
        }
    }
    spans.push(span_of(buf, cur));
    Line::from(spans)
}

fn span_of(text: String, cell: Cell) -> Span<'static> {
    let mut st = Style::default().fg(cell.color);
    if cell.bold { st = st.add_modifier(Modifier::BOLD); }
    Span::styled(text, st)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Snapshot;
    use crossterm::event::{KeyEvent, KeyModifiers};

    fn key(c: char) -> KeyEvent {
        // KeyEvent::new defaults kind = Press, state = NONE — what the
        // event loop's modal handler also expects.
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn autopilot_dodges_same_row_obstacle() {
        let mut g = GameState::new();
        g.ship_y = 5;
        g.ship_target_y = 5;
        // A small obstacle directly ahead of the ship.
        g.obstacles.push(Obstacle {
            x: (SHIP_X + 4) as f32,
            y: 5,
            kind: ObstacleKind::Small,
            hp: 1,
        });
        let snap = Snapshot::default();
        // One tick is enough — the threat heuristic should retarget.
        g.tick(&snap);
        assert_ne!(g.ship_target_y, 5,
            "autopilot should pick a different row than the one with the obstacle");
    }

    #[test]
    fn danger_obstacle_outweighs_two_small_ones() {
        let mut g = GameState::new();
        g.ship_y = 8;
        g.ship_target_y = 8;
        // Row 8: one Danger obstacle.
        g.obstacles.push(Obstacle {
            x: (SHIP_X + 5) as f32, y: 8,
            kind: ObstacleKind::Danger, hp: 1,
        });
        // Row 9: two Small obstacles.
        g.obstacles.push(Obstacle {
            x: (SHIP_X + 5) as f32, y: 9,
            kind: ObstacleKind::Small, hp: 1,
        });
        g.obstacles.push(Obstacle {
            x: (SHIP_X + 6) as f32, y: 9,
            kind: ObstacleKind::Small, hp: 1,
        });
        let snap = Snapshot::default();
        g.tick(&snap);
        // Ship should prefer the row with smalls (weight 2×2=4) over
        // the row with a Danger (weight 12).
        assert_ne!(g.ship_target_y, 8);
    }

    #[test]
    fn fire_consumes_energy_and_spawns_laser() {
        let mut g = GameState::new();
        let start = g.energy;
        let _ = g.handle_key(key(' '));
        assert!(g.energy < start, "energy should drop after firing");
        assert_eq!(g.lasers.len(), 1);
    }

    #[test]
    fn fire_blocked_when_energy_low() {
        let mut g = GameState::new();
        g.energy = LASER_COST - 0.1;
        let _ = g.handle_key(key(' '));
        assert_eq!(g.lasers.len(), 0,
            "laser should not spawn when energy is below cost");
    }

    #[test]
    fn bomb_clears_obstacles() {
        let mut g = GameState::new();
        g.energy = BOMB_COST;
        for i in 0..3 {
            g.obstacles.push(Obstacle {
                x: (SHIP_X + 5 + i) as f32, y: 4,
                kind: ObstacleKind::Small, hp: 1,
            });
        }
        let _ = g.handle_key(key('b'));
        assert!(g.obstacles.is_empty(), "bomb should clear every obstacle");
        assert!(g.score >= 15, "score should reward each cleared obstacle");
    }

    #[test]
    fn backtick_requests_close() {
        let mut g = GameState::new();
        match g.handle_key(key('`')) {
            KeyDispatch::CloseGame => {}
            _ => panic!("backtick should request CloseGame"),
        }
    }
}
