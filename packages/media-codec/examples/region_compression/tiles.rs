//! Offline side channel. Both endpoints know the negotiated dimensions and tile
//! size. A batch names its presentation time and the underlying video frame.
use flate2::{
    Compression, Decompress, FlushDecompress, Status, read::ZlibDecoder, write::ZlibEncoder,
};
use std::collections::HashSet;
use std::io::{self, Read, Write};

const SIDE: usize = 64;
const SETTLE_FRAMES: u8 = 4;
const MAX_NEW_TILES: usize = 8;
const BATCH_HEADER: usize = 24;
const FRAGMENT_HEADER: usize = 11; // tile u32, opcode u8, total/offset/length u16
const FRAGMENT_BATCH: u32 = 1 << 31;

#[derive(Clone, Copy)]
pub struct Grid {
    pub width: usize,
    pub height: usize,
}

impl Grid {
    fn count(self) -> usize {
        self.width.div_ceil(SIDE) * self.height.div_ceil(SIDE)
    }

    fn rows(self, id: usize) -> impl Iterator<Item = std::ops::Range<usize>> {
        let columns = self.width.div_ceil(SIDE);
        let x = id % columns * SIDE;
        let y = id / columns * SIDE;
        let width = SIDE.min(self.width - x);
        (y..(y + SIDE).min(self.height)).map(move |row| {
            let start = (row * self.width + x) * 4;
            start..start + width * 4
        })
    }

    fn pixel_bytes(self, id: usize) -> usize {
        self.rows(id).map(|row| row.len()).sum()
    }
}

#[derive(Clone, Copy, Default)]
struct State {
    observations: u8,
    active: bool,
    inspected: bool,
}

pub struct Sender {
    grid: Grid,
    tiles: Vec<State>,
    pending: Vec<Option<Outgoing>>,
    next_tile: usize,
    reset_flag: bool,
}

struct Outgoing {
    compressed: Vec<u8>,
    sent: usize,
}

fn batch(payload: &[u8], pts: i64, base_pts: i64, tag: u32) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(BATCH_HEADER + payload.len());
    bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&pts.to_le_bytes());
    bytes.extend_from_slice(&base_pts.to_le_bytes());
    bytes.extend_from_slice(&tag.to_le_bytes());
    bytes.extend_from_slice(payload);
    bytes
}

// Sharp neighbouring luminance transitions are a cheap proxy for strokes and
// sharp UI edges. The detector receives no fixture rectangles or annotations.
fn has_edges(grid: Grid, id: usize, bgra: &[u8]) -> bool {
    let mut edges = 0;
    for row in grid.rows(id).step_by(2) {
        let mut previous = None;
        for pixel in bgra[row].as_chunks::<4>().0.iter() {
            let brightness =
                (i32::from(pixel[0]) + 2 * i32::from(pixel[1]) + i32::from(pixel[2])) / 4;
            if previous.is_some_and(|prev: i32| (brightness - prev).abs() >= 32) {
                edges += 1;
                if edges == 12 {
                    return true;
                }
            }
            previous = Some(brightness);
        }
    }
    false
}

impl Sender {
    pub fn new(grid: Grid) -> Self {
        Self {
            grid,
            tiles: vec![State::default(); grid.count()],
            pending: (0..grid.count()).map(|_| None).collect(),
            next_tile: 0,
            reset_flag: false,
        }
    }

    pub fn observe_unpaced_reference(
        &mut self,
        current: &[u8],
        previous: Option<&[u8]>,
        pts: i64,
        base_pts: i64,
    ) -> io::Result<Vec<u8>> {
        let mut changes = Vec::new();
        let mut count = 0u32;
        let mut added = 0;
        for (id, tile) in self.tiles.iter_mut().enumerate() {
            let same = previous.is_some_and(|old| {
                self.grid
                    .rows(id)
                    .all(|row| old[row.clone()] == current[row])
            });
            if !same {
                if tile.active {
                    changes.extend_from_slice(&(id as u32).to_le_bytes());
                    changes.push(0); // invalidate
                    count += 1;
                }
                *tile = State {
                    observations: 1,
                    ..Default::default()
                };
                continue;
            }
            tile.observations = tile.observations.saturating_add(1);
            if tile.inspected || tile.observations < SETTLE_FRAMES || added >= MAX_NEW_TILES {
                continue;
            }
            tile.inspected = true;
            if has_edges(self.grid, id, current) {
                tile.active = true;
                changes.extend_from_slice(&(id as u32).to_le_bytes());
                changes.push(1); // exact BGRA replacement
                for row in self.grid.rows(id) {
                    changes.extend_from_slice(&current[row]);
                }
                added += 1;
                count += 1;
            }
        }
        if count == 0 {
            return Ok(Vec::new());
        }
        let mut compressed = ZlibEncoder::new(Vec::new(), Compression::fast());
        compressed.write_all(&changes)?;
        let compressed = compressed.finish()?;
        Ok(batch(&compressed, pts, base_pts, count))
    }

    /// Reliable ordered side channel. Clears are mandatory. Optional data uses
    /// only remaining wire bytes, including headers. A caller granting less
    /// than 36 bytes per tick needs to accumulate credit until one byte fits.
    pub fn observe_budgeted(
        &mut self,
        current: &[u8],
        previous: Option<&[u8]>,
        pts: i64,
        base_pts: i64,
        budget: usize,
    ) -> io::Result<Vec<u8>> {
        let mut payload = Vec::new();
        let mut count = 0;
        let mut reset = false;
        for (id, tile) in self.tiles.iter_mut().enumerate() {
            let same = previous.is_some_and(|old| {
                self.grid
                    .rows(id)
                    .all(|row| old[row.clone()] == current[row])
            });
            if same {
                tile.observations = tile.observations.saturating_add(1);
                continue;
            }
            let partial = self.pending[id].take();
            if tile.active || partial.is_some_and(|p| p.sent != 0) {
                if base_pts == pts {
                    reset = true;
                } else {
                    payload.extend_from_slice(&(id as u32).to_le_bytes());
                    payload.push(0);
                    count += 1;
                }
            }
            *tile = State {
                observations: 1,
                ..Default::default()
            };
        }
        if reset {
            // Piggyback on a fresh video frame. A side-channel-only update against
            // an older base cannot retroactively change that video's flags.
            self.reset_flag = true;
            for (tile, partial) in self.tiles.iter_mut().zip(&mut self.pending) {
                if partial.take().is_some() || tile.active {
                    tile.active = false;
                    tile.inspected = false;
                }
            }
            return Ok(Vec::new());
        }
        let tile_count = self.tiles.len();
        let start = self.next_tile;
        let mut prepared = 0;
        for step in 0..tile_count {
            let space = budget.saturating_sub(BATCH_HEADER + payload.len() + FRAGMENT_HEADER);
            if space == 0 {
                break;
            }
            let id = (start + step) % tile_count;
            let tile = &mut self.tiles[id];
            if tile.active || tile.observations < SETTLE_FRAMES {
                continue;
            }
            if self.pending[id].is_none() {
                if tile.inspected || prepared >= MAX_NEW_TILES {
                    continue;
                }
                tile.inspected = true;
                if !has_edges(self.grid, id, current) {
                    continue;
                }
                let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
                for row in self.grid.rows(id) {
                    encoder.write_all(&current[row])?;
                }
                self.pending[id] = Some(Outgoing {
                    compressed: encoder.finish()?,
                    sent: 0,
                });
                prepared += 1;
            }
            let outgoing = self.pending[id].as_mut().expect("prepared tile");
            let total = outgoing.compressed.len();
            let length = space.min(total - outgoing.sent);
            payload.extend_from_slice(&(id as u32).to_le_bytes());
            payload.push(1);
            payload.extend_from_slice(&(total as u16).to_le_bytes());
            payload.extend_from_slice(&(outgoing.sent as u16).to_le_bytes());
            payload.extend_from_slice(&(length as u16).to_le_bytes());
            payload.extend_from_slice(&outgoing.compressed[outgoing.sent..outgoing.sent + length]);
            outgoing.sent += length;
            if outgoing.sent == total {
                self.pending[id] = None;
                tile.active = true;
                self.next_tile = (id + 1) % tile_count;
            } else {
                // Finish this bounded tile before opening another. Every usable
                // grant advances the prefix; if content changes, its cancellation
                // retires it and the stable candidates can immediately use credit.
                self.next_tile = id;
            }
            count += 1;
        }
        if count == 0 {
            return Ok(Vec::new());
        }
        Ok(batch(&payload, pts, base_pts, FRAGMENT_BATCH | count))
    }

    /// Consume immediately after observe_budgeted, when serializing its video header.
    pub fn take_reset_flag(&mut self) -> bool {
        std::mem::take(&mut self.reset_flag)
    }
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid tile batch or video reference",
    )
}

pub struct Receiver {
    grid: Grid,
    patches: Vec<Option<Vec<u8>>>,
    incoming: Vec<Option<Incoming>>,
    last_fragment_pts: Option<i64>,
    pub installs: usize,
    pub invalidations: usize,
}

struct Incoming {
    compressed: Vec<u8>,
    total: usize,
}

enum Update<'a> {
    Clear(usize),
    Partial(usize, usize, &'a [u8]),
    Install(usize, Vec<u8>),
}

fn take<'a>(input: &mut &'a [u8], n: usize) -> io::Result<&'a [u8]> {
    let (value, rest) = input.split_at_checked(n).ok_or_else(invalid)?;
    *input = rest;
    Ok(value)
}

fn u16_field(input: &mut &[u8]) -> io::Result<usize> {
    Ok(u16::from_le_bytes(take(input, 2)?.try_into().unwrap()) as usize)
}

fn decode_tile(prefix: &[u8], end: &[u8], raw_size: usize) -> io::Result<Vec<u8>> {
    let compressed = [prefix, end].concat();
    // One extra byte permits zlib to check its checksum at an output boundary
    // and detects a tile whose decompressed size exceeds its geometry.
    let mut pixels = vec![0; raw_size + 1];
    let mut zlib = Decompress::new(true);
    let status = zlib.decompress(&compressed, &mut pixels, FlushDecompress::Finish)?;
    if status != Status::StreamEnd
        || zlib.total_out() != raw_size as u64
        || zlib.total_in() != compressed.len() as u64
    {
        return Err(invalid());
    }
    pixels.truncate(raw_size);
    Ok(pixels)
}

impl Receiver {
    pub fn new(grid: Grid) -> Self {
        Self {
            grid,
            patches: vec![None; grid.count()],
            incoming: (0..grid.count()).map(|_| None).collect(),
            last_fragment_pts: None,
            installs: 0,
            invalidations: 0,
        }
    }

    pub fn apply(&mut self, batch: &[u8], pts: i64, base_pts: i64) -> io::Result<()> {
        if batch.len() < 24
            || u32::from_le_bytes(batch[..4].try_into().unwrap()) as usize != batch.len() - 24
            || batch[4..12] != pts.to_le_bytes()
            || batch[12..20] != base_pts.to_le_bytes()
        {
            return Err(invalid());
        }
        let tag = u32::from_le_bytes(batch[20..24].try_into().unwrap());
        if tag & FRAGMENT_BATCH != 0 {
            return self.apply_fragments(
                &batch[BATCH_HEADER..],
                (tag & !FRAGMENT_BATCH) as usize,
                pts,
            );
        }
        let count = tag as usize;
        if count > self.patches.len() {
            return Err(invalid());
        }
        let mut compressed = ZlibDecoder::new(&batch[24..]);
        for _ in 0..count {
            let mut entry = [0u8; 5];
            compressed.read_exact(&mut entry)?;
            let id = u32::from_le_bytes(entry[..4].try_into().unwrap()) as usize;
            let target = self.patches.get_mut(id).ok_or_else(invalid)?;
            match entry[4] {
                0 if target.is_some() => {
                    *target = None;
                    self.invalidations += 1;
                }
                1 if target.is_none() => {
                    let mut pixels = vec![0; self.grid.rows(id).map(|r| r.len()).sum()];
                    compressed.read_exact(&mut pixels)?;
                    *target = Some(pixels);
                    self.installs += 1;
                }
                _ => return Err(invalid()),
            }
        }
        if compressed.read(&mut [0])? != 0 {
            return Err(invalid());
        }
        Ok(())
    }

    fn apply_fragments(&mut self, mut data: &[u8], count: usize, pts: i64) -> io::Result<()> {
        if count == 0
            || count > self.patches.len()
            || count > data.len() / 5
            || self.last_fragment_pts.is_some_and(|last| pts <= last)
        {
            return Err(invalid());
        }
        // Validate the entire batch before changing the visible image or pending
        // buffers. A truncated second operation cannot commit the first.
        let mut ids = HashSet::with_capacity(count);
        let mut updates = Vec::with_capacity(count);
        for _ in 0..count {
            let id = u32::from_le_bytes(take(&mut data, 4)?.try_into().unwrap()) as usize;
            if id >= self.patches.len() || !ids.insert(id) {
                return Err(invalid());
            }
            match take(&mut data, 1)?[0] {
                0 if self.patches[id].is_some() || self.incoming[id].is_some() => {
                    updates.push(Update::Clear(id))
                }
                1 if self.patches[id].is_none() => {
                    let total = u16_field(&mut data)?;
                    let offset = u16_field(&mut data)?;
                    let length = u16_field(&mut data)?;
                    let part = take(&mut data, length)?;
                    let raw_size = self.grid.pixel_bytes(id);
                    if length == 0 || offset + length > total || total > raw_size * 2 + 64 {
                        return Err(invalid());
                    }
                    let prefix = match &self.incoming[id] {
                        Some(p) if p.total == total && p.compressed.len() == offset => {
                            p.compressed.as_slice()
                        }
                        None if offset == 0 => &[],
                        _ => return Err(invalid()),
                    };
                    if offset + length == total {
                        updates.push(Update::Install(id, decode_tile(prefix, part, raw_size)?));
                    } else {
                        updates.push(Update::Partial(id, total, part));
                    }
                }
                _ => return Err(invalid()),
            }
        }
        if !data.is_empty() {
            return Err(invalid());
        }
        for update in updates {
            match update {
                Update::Clear(id) => {
                    self.incoming[id] = None;
                    if self.patches[id].take().is_some() {
                        self.invalidations += 1;
                    }
                }
                Update::Partial(id, total, part) => {
                    self.incoming[id]
                        .get_or_insert_with(|| Incoming {
                            compressed: Vec::with_capacity(total),
                            total,
                        })
                        .compressed
                        .extend_from_slice(part);
                }
                Update::Install(id, pixels) => {
                    self.incoming[id] = None;
                    self.patches[id] = Some(pixels);
                    self.installs += 1;
                }
            }
        }
        self.last_fragment_pts = Some(pts);
        Ok(())
    }

    pub fn has_patches(&self) -> bool {
        self.installs > self.invalidations
    }

    /// Apply the reset flag to its decoded video before composition. Partial
    /// tiles have never been visible, so only installed patches count as clears.
    pub fn reset_patches(&mut self) {
        for (patch, partial) in self.patches.iter_mut().zip(&mut self.incoming) {
            if patch.take().is_some() {
                self.invalidations += 1;
            }
            *partial = None;
        }
    }

    pub fn composite(&self, output: &mut [u8]) {
        for (id, patch) in self.patches.iter().enumerate() {
            if let Some(patch) = patch {
                let mut input = patch.as_slice();
                for row in self.grid.rows(id) {
                    let (pixels, rest) = input.split_at(row.len());
                    output[row].copy_from_slice(pixels);
                    input = rest;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn high_and_low_entropy_tiles(grid: Grid) -> Vec<u8> {
        let mut rng = 19u32;
        let mut pixels = Vec::new();
        for i in 0..grid.width * grid.height {
            let mut color = [((i % 2) * 255) as u8; 4];
            if i % grid.width < SIDE {
                for channel in &mut color {
                    rng ^= rng << 13;
                    rng ^= rng >> 17;
                    rng ^= rng << 5;
                    *channel = rng as u8;
                }
            }
            pixels.extend_from_slice(&color);
        }
        pixels
    }

    fn settle_without_credit(tx: &mut Sender, pixels: &[u8]) {
        for pts in 0..8 {
            let previous = (pts > 0).then_some(pixels);
            assert!(
                tx.observe_budgeted(pixels, previous, pts, 0, 0)
                    .unwrap()
                    .is_empty()
            );
        }
        assert!(tx.pending.iter().all(Option::is_none));
        assert!(tx.tiles.iter().all(|t| !t.inspected));
    }

    #[test]
    fn minimum_fragment_budget_reassembles_exact_odd_edge_tile() {
        let grid = Grid {
            width: 19,
            height: 9,
        };
        let pixels = high_and_low_entropy_tiles(grid);
        let mut tx = Sender::new(grid);
        let mut rx = Receiver::new(grid);
        settle_without_credit(&mut tx, &pixels);
        let minimum = BATCH_HEADER + FRAGMENT_HEADER + 1;
        assert!(
            tx.observe_budgeted(&pixels, Some(&pixels), 8, 0, minimum - 1)
                .unwrap()
                .is_empty()
        );
        for pts in 9..2000 {
            let packet = tx
                .observe_budgeted(&pixels, Some(&pixels), pts, 0, minimum)
                .unwrap();
            assert_eq!(packet.len(), minimum);
            rx.apply(&packet, pts, 0).unwrap();
            let mut displayed = vec![0; pixels.len()];
            rx.composite(&mut displayed);
            if rx.has_patches() {
                assert_eq!(displayed, pixels);
                assert_eq!(rx.installs, 1);
                assert!(
                    tx.observe_budgeted(&pixels, Some(&pixels), pts + 1, 0, 1000)
                        .unwrap()
                        .is_empty()
                );
                return;
            }
            assert!(displayed.iter().all(|&x| x == 0));
        }
        panic!("an eligible tile never completed");
    }

    #[test]
    fn started_tile_finishes_before_opening_more_and_each_candidate_completes() {
        let grid = Grid {
            width: SIDE * 2,
            height: SIDE,
        };
        let pixels = high_and_low_entropy_tiles(grid);
        let mut tx = Sender::new(grid);
        let mut rx = Receiver::new(grid);
        settle_without_credit(&mut tx, &pixels);
        for pts in 8..120 {
            let packet = tx
                .observe_budgeted(&pixels, Some(&pixels), pts, 0, 250)
                .unwrap();
            assert!(packet.len() <= 250);
            assert!(!tx.take_reset_flag());
            if !packet.is_empty() {
                rx.apply(&packet, pts, 0).unwrap();
            }
            if rx.patches[0].is_none() {
                assert!(rx.patches[1].is_none() && rx.incoming[1].is_none());
            }
            assert!(rx.incoming.iter().flatten().count() <= 1);
            if rx.installs == 2 {
                let mut displayed = vec![0; pixels.len()];
                rx.composite(&mut displayed);
                assert_eq!(displayed, pixels);
                return;
            }
        }
        panic!("a ready tile starved");
    }

    // Two cheap tiles complete in the first grant, followed by a prefix of a
    // much larger tile. Changing either a visible or an in-flight tile must
    // invalidate the correct generation without exposing partial pixels.
    fn staged_scene() -> (Grid, Vec<u8>, Sender, Receiver, Vec<u8>) {
        let grid = Grid {
            width: SIDE * 3,
            height: SIDE,
        };
        let mut pixels = high_and_low_entropy_tiles(grid);
        for row in pixels.chunks_exact_mut(grid.width * 4) {
            row.rotate_left(SIDE * 4);
        }
        let mut tx = Sender::new(grid);
        let mut rx = Receiver::new(grid);
        settle_without_credit(&mut tx, &pixels);
        let initial = tx
            .observe_budgeted(&pixels, Some(&pixels), 8, 0, 600)
            .unwrap();
        rx.apply(&initial, 8, 0).unwrap();
        assert_eq!(rx.installs, 2);
        assert!(rx.incoming[2].is_some());
        (grid, pixels, tx, rx, initial)
    }

    #[test]
    fn fresh_video_clears_visible_and_partial_tiles_atomically_then_repairs_unchanged_tiles() {
        for (budget, changed_tile) in [(0, 0), (4096, 2)] {
            let (grid, pixels, mut tx, mut rx, _) = staged_scene();
            let mut changed = pixels.clone();
            for row in grid.rows(changed_tile) {
                changed[row].fill(29);
            }
            let packet = tx
                .observe_budgeted(&changed, Some(&pixels), 9, 9, budget)
                .unwrap();
            assert!(packet.is_empty());
            assert!(tx.take_reset_flag());
            assert!(!tx.take_reset_flag());
            rx.reset_patches();
            assert_eq!(rx.invalidations, 2);
            assert!(!rx.has_patches());
            assert!(rx.incoming.iter().all(Option::is_none));
            let mut display = vec![13; changed.len()];
            rx.composite(&mut display);
            assert!(display.iter().all(|&v| v == 13));
            let packet = tx
                .observe_budgeted(&changed, Some(&changed), 10, 9, 50_000)
                .unwrap();
            assert!(!tx.take_reset_flag());
            rx.apply(&packet, 10, 9).unwrap();
            assert_eq!(rx.installs, 4);
            assert!(rx.patches[changed_tile].is_none());
            for id in 0..3 {
                if id != changed_tile {
                    assert!(rx.patches[id].is_some());
                }
            }
        }
    }

    #[test]
    fn side_channel_only_change_cancels_even_without_credit() {
        let (_, pixels, mut tx, mut rx, initial) = staged_scene();
        let blank = vec![0; pixels.len()];
        let clear = tx.observe_budgeted(&blank, Some(&pixels), 9, 0, 0).unwrap();
        assert!(!tx.take_reset_flag());
        assert_eq!(clear.len(), BATCH_HEADER + 3 * 5);
        rx.apply(&clear, 9, 0).unwrap();
        assert_eq!(rx.invalidations, 2);
        assert!(!rx.has_patches());
        assert!(rx.incoming.iter().all(Option::is_none));
        assert!(rx.apply(&initial, 8, 0).is_err());
    }

    #[test]
    fn corrupt_batch_does_not_commit_a_valid_prefix() {
        let grid = Grid {
            width: SIDE * 2,
            height: SIDE,
        };
        let pixels = high_and_low_entropy_tiles(grid);
        let mut tx = Sender::new(grid);
        let mut rx = Receiver::new(grid);
        settle_without_credit(&mut tx, &pixels);
        let packet = tx
            .observe_budgeted(&pixels, Some(&pixels), 8, 0, 50_000)
            .unwrap();
        let mut corrupt = packet.clone();
        *corrupt.last_mut().unwrap() ^= 255;
        assert!(rx.apply(&corrupt, 8, 0).is_err());
        assert_eq!(rx.installs, 0);
        assert!(rx.incoming.iter().all(Option::is_none));
        rx.apply(&packet, 8, 0).unwrap();
        assert_eq!(rx.installs, 2);
    }

    #[test]
    fn settled_edge_tiles_round_trip_and_disappear_on_first_change() {
        let grid = Grid {
            width: 70,
            height: 18,
        };
        let mut tx = Sender::new(grid);
        let mut rx = Receiver::new(grid);
        let stripes: Vec<_> = (0..grid.width * grid.height)
            .flat_map(|i| [((i % 2) * 255) as u8; 4])
            .collect();
        for n in 0..4 {
            let batch = tx
                .observe_unpaced_reference(&stripes, (n > 0).then_some(&stripes), n, 0)
                .unwrap();
            if n < 3 {
                assert!(batch.is_empty());
            } else {
                assert!(rx.apply(&batch, n, 1).is_err());
                rx.apply(&batch, n, 0).unwrap();
            }
        }
        let mut display = vec![0; stripes.len()];
        assert!(rx.has_patches());
        rx.composite(&mut display);
        assert_eq!(display, stripes);
        let blank = vec![19; stripes.len()];
        let batch = tx
            .observe_unpaced_reference(&blank, Some(&stripes), 4, 4)
            .unwrap();
        rx.apply(&batch, 4, 4).unwrap();
        assert!(!rx.has_patches());
        display.fill(255);
        rx.composite(&mut display);
        assert!(display.iter().all(|x| *x == 255));
        assert_eq!(rx.invalidations, 2);
    }
}
