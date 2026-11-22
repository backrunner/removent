//! Deterministic BGRA inputs for the region-compression experiment. A frame
//! depends only on its index, including after a skipped frame or a restart.

use std::io::Cursor;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scene {
    Mixed,
    Scroll,
    Game,
    Static,
    LateEdit,
}

type Rect = (u32, u32, u32, u32);
pub const MIXED_TEXT_EDIT_FRAME: u32 = 24;
pub const LATE_EDIT_FRAME: u32 = 300;
const LINE_HEIGHT: usize = 30;

pub struct Fixture {
    scene: Scene,
    width: u32,
    height: u32,
    paper: Vec<u8>,
    landscape: Vec<u8>,
}

impl Fixture {
    pub fn new(scene: Scene, width: u32, height: u32) -> Result<Self, String> {
        if width < 4 || height < 4 {
            return Err("the fixture needs at least 4 × 4 pixels".into());
        }
        let mut fixture = Self {
            scene,
            width,
            height,
            paper: Vec::new(),
            landscape: Vec::new(),
        };
        if scene != Scene::Game {
            fixture.paper = document(width, height)?;
        }
        if matches!(scene, Scene::Mixed | Scene::Game) {
            let (_, _, w, h) = fixture.motion_rect();
            fixture.landscape = panorama(w, h);
        }
        Ok(fixture)
    }

    pub fn motion_rect(&self) -> Rect {
        match self.scene {
            // One video window covering roughly 20% of the desktop.
            Scene::Mixed => (
                self.width / 2,
                self.height / 3,
                self.width * 2 / 5,
                self.height / 2,
            ),
            Scene::Scroll | Scene::Game | Scene::LateEdit => (0, 0, self.width, self.height),
            Scene::Static => (0, 0, 0, 0),
        }
    }

    pub fn text_rect(&self) -> Option<Rect> {
        match self.scene {
            Scene::Mixed => Some((0, 0, self.width / 2, self.height)),
            Scene::Scroll | Scene::Static | Scene::LateEdit => {
                Some((0, 0, self.width, self.height))
            }
            Scene::Game => None,
        }
    }

    pub fn render(&self, index: u32) -> Vec<u8> {
        let stride = self.width as usize * 4;
        let mut frame = vec![0; stride * self.height as usize];
        if !self.paper.is_empty() {
            let late_edit = self.scene == Scene::LateEdit && index >= LATE_EDIT_FRAME;
            let offset = match self.scene {
                Scene::Scroll => index as usize * 5,
                Scene::LateEdit if late_edit => (index - LATE_EDIT_FRAME) as usize * 5,
                _ => 0,
            };
            let rows = self.paper.len() / stride;
            for (y, row) in frame.chunks_exact_mut(stride).enumerate() {
                let mut source_y = (y + offset) % rows;
                // Replace the settings line with the following status line.
                // Scroll the edited document from this point onwards.
                if late_edit && (LINE_HEIGHT..2 * LINE_HEIGHT).contains(&source_y) {
                    source_y += LINE_HEIGHT;
                }
                let pos = source_y * stride;
                row.copy_from_slice(&self.paper[pos..pos + stride]);
                if late_edit {
                    // A simultaneous theme flip changes even blank or repeated
                    // areas. Every previously visible correction must expire.
                    for pixel in row.as_chunks_mut::<4>().0 {
                        for channel in &mut pixel[..3] {
                            *channel = 255 - *channel;
                        }
                    }
                }
            }
        }
        // Replace the first left-column Chinese settings line with the
        // network-status sentence, after the original has settled. A viewer
        // retaining an old text patch will visibly show stale glyphs here.
        if self.scene == Scene::Mixed && index >= MIXED_TEXT_EDIT_FRAME {
            let edit_width = (self.width / 2).min(520) as usize * 4;
            for y in LINE_HEIGHT..(2 * LINE_HEIGHT).min(self.height as usize) {
                let src = (y + LINE_HEIGHT) * stride;
                let dst = y * stride;
                frame[dst..dst + edit_width].copy_from_slice(&self.paper[src..src + edit_width]);
            }
        }
        if !self.landscape.is_empty() {
            let (left, top, w, h) = self.motion_rect();
            let span = w as usize * 4;
            for y in 0..h {
                // Distant scenery, buildings and foreground move by different
                // amounts. A single copy/scroll operation cannot reproduce all of it.
                let speed = 2 + 2 * u64::from(y * 2 >= h) + 5 * u64::from(y * 4 >= h * 3);
                let x = (u64::from(index) * speed % u64::from(w)) as usize * 4;
                let src = y as usize * span;
                let dst = (top + y) as usize * stride + left as usize * 4;
                frame[dst..dst + span - x].copy_from_slice(&self.landscape[src + x..src + span]);
                frame[dst + span - x..dst + span].copy_from_slice(&self.landscape[src..src + x]);
            }
        }
        frame
    }
}

// The source is the repository's existing uncompressed-UI reference PNG,
// generated by scripts/fixtures/readability_fixture.swift. Repack its light and
// dark rows at their native pixel size; tiny strokes remain actual text.
fn document(width: u32, height: u32) -> Result<Vec<u8>, String> {
    const REFERENCE: &[u8] =
        include_bytes!("../../../../docs/benchmarks/2026-09-20-readability/source.png");
    let mut reader = png::Decoder::new(Cursor::new(REFERENCE))
        .read_info()
        .map_err(|e| e.to_string())?;
    let mut source = vec![
        0;
        reader
            .output_buffer_size()
            .ok_or("invalid reference size")?
    ];
    let image = reader.next_frame(&mut source).map_err(|e| e.to_string())?;
    if (image.width, image.height, image.color_type, image.bit_depth)
        != (1920, 1080, png::ColorType::Rgba, png::BitDepth::Eight)
    {
        return Err("unexpected readability reference format".into());
    }
    // Y coordinates of the eleven complete lines, including 10–22 px fonts.
    const LINE_TOPS: [usize; 11] = [54, 110, 160, 210, 260, 310, 360, 483, 540, 598, 654];
    const PERIOD: usize = LINE_HEIGHT * LINE_TOPS.len();
    let paper_height = (height as usize).div_ceil(PERIOD) * PERIOD;
    let mut paper = vec![0; width as usize * paper_height * 4];
    for (y, row) in paper.chunks_exact_mut(width as usize * 4).enumerate() {
        let sy = LINE_TOPS[y % PERIOD / LINE_HEIGHT] + y % LINE_HEIGHT;
        for (x, pixel) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            // Repeat columns on wider displays, alternating paper and dark theme.
            let sx = 22 + x % 520 + (x / 520 % 2) * 960;
            let pos = (sy * 1920 + sx) * 4;
            pixel.copy_from_slice(&[source[pos + 2], source[pos + 1], source[pos], 255]);
        }
    }
    Ok(paper)
}

// A scrolling 2D game-style cityscape. Edges and deterministic fine textures
// exercise motion compensation with correlated content across successive frames.
fn panorama(width: u32, height: u32) -> Vec<u8> {
    let mut world = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            let block = x / 43;
            let tile = hash(block);
            let texture = hash((y / 3).wrapping_mul(911).wrapping_add(x / 4));
            let grain = (texture & 15) as u8;
            let roof = height / 6 + tile % (height / 3).max(1);
            let (r, g, b) = if y * 4 >= height * 3 {
                // Road markings and textured paving.
                if y.abs_diff(height * 7 / 8) < 2 && x % 83 < 46 {
                    (225, 198, 99)
                } else {
                    (53 + grain, 56 + grain, 64 + grain)
                }
            } else if y >= roof {
                if x % 13 < 6 && (y - roof) % 14 < 5 {
                    (165 + (tile % 60) as u8, 152 + grain * 2, 98 + grain)
                } else {
                    (51 + (tile % 50) as u8 + grain, 58 + grain, 79 + grain)
                }
            } else {
                let cloud = hash(x / 9 + y / 5 * 701) % 23;
                let haze = (y * 45 / height + cloud) as u8;
                (50 + haze, 107 + haze, 156 + haze)
            };
            world.extend_from_slice(&[b, g, r, 255]);
        }
    }
    world
}

fn hash(mut value: u32) -> u32 {
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb_352d);
    value ^ (value >> 15)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn late_edit_changes_every_tile_after_a_still_document() {
        let (width, height) = (1280, 720);
        let scene = Fixture::new(Scene::LateEdit, width, height).unwrap();
        let initial = Fixture::new(Scene::Static, width, height)
            .unwrap()
            .render(0);
        assert_eq!(initial, scene.render(0));
        assert_eq!(initial, scene.render(LATE_EDIT_FRAME - 1));
        let changed = scene.render(LATE_EDIT_FRAME);
        let (w, h) = (width as usize, height as usize);
        for y in (0..h).step_by(64) {
            for x in (0..w).step_by(64) {
                assert!((y..(y + 64).min(h)).any(|row| {
                    let start = (row * w + x) * 4;
                    let end = (row * w + (x + 64).min(w)) * 4;
                    initial[start..end] != changed[start..end]
                }));
            }
        }
        let next = scene.render(LATE_EDIT_FRAME + 1);
        let amount = 5 * w * 4;
        assert_eq!(changed[amount..], next[..next.len() - amount]);
    }
}
