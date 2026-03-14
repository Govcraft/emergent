---cargo
[dependencies]
emergent-client = "0.10.5"
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
rayon = "1"
---

//! Gray-Scott reaction-diffusion as a Rust script.
//!
//! Two chemicals (U, V) diffuse and react on a grid. Feed/kill rates
//! select the pattern: spots, coral, maze, mitosis, etc.
//! Uses rayon to parallelize the grid computation across CPU cores.

use emergent_client::{EmergentHandler, EmergentMessage};
use rayon::prelude::*;
use serde_json::json;
use tokio::signal::unix::{SignalKind, signal};

const ROWS: usize = 100;
const COLS: usize = 140;
const CELLS: usize = ROWS * COLS;
const DU: f64 = 0.16;
const DV: f64 = 0.08;
const SUBSTEPS: usize = 16;

struct World {
    u: Vec<f64>,
    v: Vec<f64>,
    feed: f64,
    kill: f64,
    generation: u64,
}

impl World {
    fn new() -> Self {
        Self {
            u: vec![1.0; CELLS],
            v: vec![0.0; CELLS],
            feed: 0.0,
            kill: 0.0,
            generation: 0,
        }
    }

    fn seed(&mut self, preset: &str) {
        let (f, k) = match preset {
            "mitosis" => (0.0367, 0.0649),
            "coral"   => (0.0545, 0.062),
            "maze"    => (0.029,  0.057),
            "holes"   => (0.039,  0.058),
            "waves"   => (0.014,  0.045),
            "spots"   => (0.03,   0.06),
            "worms"   => (0.078,  0.061),
            _         => (0.0545, 0.062),
        };
        self.feed = f;
        self.kill = k;
        self.generation = 0;
        self.u.fill(1.0);
        self.v.fill(0.0);

        // Seed center block
        let cr = ROWS / 2;
        let cc = COLS / 2;
        let mut rng = simple_rng(42);
        for r in cr.saturating_sub(10)..std::cmp::min(cr + 10, ROWS) {
            for c in cc.saturating_sub(10)..std::cmp::min(cc + 10, COLS) {
                let idx = r * COLS + c;
                self.u[idx] = 0.5 + rng_f64(&mut rng) * 0.1 - 0.05;
                self.v[idx] = 0.25 + rng_f64(&mut rng) * 0.1 - 0.05;
            }
        }

        // Scattered seeds
        for _ in 0..5 {
            let sr = 10 + (rng_u64(&mut rng) as usize % (ROWS - 20));
            let sc = 10 + (rng_u64(&mut rng) as usize % (COLS - 20));
            for dr in -3i32..=3 {
                for dc in -3i32..=3 {
                    let r = (sr as i32 + dr) as usize;
                    let c = (sc as i32 + dc) as usize;
                    if r < ROWS && c < COLS {
                        let idx = r * COLS + c;
                        self.u[idx] = 0.5 + rng_f64(&mut rng) * 0.1 - 0.05;
                        self.v[idx] = 0.25 + rng_f64(&mut rng) * 0.1 - 0.05;
                    }
                }
            }
        }
    }

    fn step(&mut self) {
        let u = &self.u;
        let v = &self.v;
        let feed = self.feed;
        let kill = self.kill;

        let (new_u, new_v): (Vec<f64>, Vec<f64>) = (0..ROWS)
            .into_par_iter()
            .flat_map_iter(|r| {
                let up = if r == 0 { ROWS - 1 } else { r - 1 };
                let dn = if r == ROWS - 1 { 0 } else { r + 1 };
                (0..COLS).map(move |c| {
                    let lt = if c == 0 { COLS - 1 } else { c - 1 };
                    let rt = if c == COLS - 1 { 0 } else { c + 1 };

                    let idx = r * COLS + c;
                    let uv = u[idx];
                    let vv = v[idx];
                    let uvv2 = uv * vv * vv;

                    let lap_u = u[up * COLS + c] + u[dn * COLS + c]
                        + u[r * COLS + lt] + u[r * COLS + rt] - 4.0 * uv;
                    let lap_v = v[up * COLS + c] + v[dn * COLS + c]
                        + v[r * COLS + lt] + v[r * COLS + rt] - 4.0 * vv;

                    let nu = (uv + DU * lap_u - uvv2 + feed * (1.0 - uv)).clamp(0.0, 1.0);
                    let nv = (vv + DV * lap_v + uvv2 - (feed + kill) * vv).clamp(0.0, 1.0);
                    (nu, nv)
                })
            })
            .unzip();

        self.u = new_u;
        self.v = new_v;
    }

    fn to_payload(&self) -> serde_json::Value {
        let cells: Vec<u8> = self.v.iter().map(|&vv| (vv * 255.0).min(255.0) as u8).collect();
        json!({
            "metric": "rd",
            "generation": self.generation,
            "rows": ROWS,
            "cols": COLS,
            "cells": cells,
        })
    }

    fn is_empty(&self) -> bool {
        self.generation == 0 && self.v.iter().all(|&vv| vv == 0.0)
    }
}

// Minimal xorshift64 PRNG (no external dep needed)
fn simple_rng(seed: u64) -> u64 { seed }
fn rng_u64(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}
fn rng_f64(state: &mut u64) -> f64 {
    (rng_u64(state) % 10000) as f64 / 10000.0
}

fn unwrap_stdout(payload: &serde_json::Value) -> serde_json::Value {
    if let Some(stdout) = payload.get("stdout").and_then(|s| s.as_str()) {
        serde_json::from_str(stdout).unwrap_or_else(|_| payload.clone())
    } else {
        payload.clone()
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let name = std::env::var("EMERGENT_NAME").unwrap_or_else(|_| "world".to_string());

    let handler = match EmergentHandler::connect(&name).await {
        Ok(h) => h,
        Err(e) => {
            eprintln!("Failed to connect: {e}");
            std::process::exit(1);
        }
    };

    let mut stream = match handler.subscribe(&["rd.seed", "rd.tick"]).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to subscribe: {e}");
            std::process::exit(1);
        }
    };

    let mut sigterm = signal(SignalKind::terminate())?;
    let mut world = World::new();

    loop {
        tokio::select! {
            _ = sigterm.recv() => {
                let _ = handler.disconnect().await;
                break;
            }
            msg = stream.next() => {
                match msg {
                    Some(msg) => {
                        let p = unwrap_stdout(msg.payload());
                        let msg_type = msg.message_type().to_string();

                        if msg_type == "rd.seed" {
                            let preset = p.get("preset")
                                .and_then(|v| v.as_str())
                                .unwrap_or("coral");
                            world.seed(preset);
                            let output = EmergentMessage::new("rd.frame")
                                .with_causation_id(msg.id())
                                .with_payload(world.to_payload());
                            let _ = handler.publish(output).await;
                        } else if msg_type == "rd.tick" && !world.is_empty() {
                            for _ in 0..SUBSTEPS {
                                world.step();
                                world.generation += 1;
                            }
                            let output = EmergentMessage::new("rd.frame")
                                .with_causation_id(msg.id())
                                .with_payload(world.to_payload());
                            let _ = handler.publish(output).await;
                        }
                    }
                    None => break,
                }
            }
        }
    }

    Ok(())
}
