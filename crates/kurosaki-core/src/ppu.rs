use crate::mapper::NametableMirroring;
use crate::trace::{TraceConfig, TraceEvent, TraceSink};
use serde::{Deserialize, Serialize};

pub const NES_WIDTH: usize = 256;
pub const NES_HEIGHT: usize = 240;
const DOTS_PER_SCANLINE: u16 = 341;
const SCANLINES_PER_NTSC_FRAME: i16 = 262;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PpuFrameStats {
    pub frame: u64,
    pub rendered_scanlines: u32,
    pub pattern_fetches: u64,
    pub sprite0_hit_candidates: u64,
    pub sprite_overflow_candidates: u64,
    pub pixel_hash: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PpuState {
    pub ctrl: u8,
    pub mask: u8,
    pub status: u8,
    pub oam_addr: u8,
    pub scroll_latch: bool,
    pub addr_latch: bool,
    pub vram_addr: u16,
    pub temp_addr: u16,
    pub fine_x: u8,
    pub scroll_x: u8,
    pub scroll_y: u8,
    pub nametable_mirroring: NametableMirroring,
    pub palette: Vec<u8>,
    pub oam: Vec<u8>,
    pub vram: Vec<u8>,
    pub frame_buffer: Vec<u8>,
    pub scanline: i16,
    pub dot: u16,
    pub rendered_frames: u64,
    pub rendered_scanlines: u64,
    pub pattern_fetches: u64,
    pub last_pixel_hash: u64,
    pub sprite0_hit_candidates: u64,
    pub sprite_overflow_candidates: u64,
    pub ctrl_writes: u64,
    pub mask_writes: u64,
    pub scroll_writes: u64,
    pub addr_writes: u64,
    pub data_writes: u64,
    pub data_writes_outside_vblank: u64,
    pub ctrl_writes_while_rendering: u64,
    pub mask_writes_while_rendering: u64,
    pub scroll_writes_while_rendering: u64,
    pub addr_writes_while_rendering: u64,
    pub data_writes_while_rendering: u64,
    pub status_reads: u64,
    pub vblank_count: u64,
    #[serde(default)]
    pub suppress_next_vblank_status: bool,
}

impl Default for PpuState {
    fn default() -> Self {
        Self {
            ctrl: 0,
            mask: 0,
            status: 0x00,
            oam_addr: 0,
            scroll_latch: false,
            addr_latch: false,
            vram_addr: 0,
            temp_addr: 0,
            fine_x: 0,
            scroll_x: 0,
            scroll_y: 0,
            nametable_mirroring: NametableMirroring::FourScreen,
            palette: vec![0; 32],
            oam: vec![0; 256],
            vram: vec![0; 0x4000],
            frame_buffer: vec![0; NES_WIDTH * NES_HEIGHT],
            scanline: 0,
            dot: 0,
            rendered_frames: 0,
            rendered_scanlines: 0,
            pattern_fetches: 0,
            last_pixel_hash: 0,
            sprite0_hit_candidates: 0,
            sprite_overflow_candidates: 0,
            ctrl_writes: 0,
            mask_writes: 0,
            scroll_writes: 0,
            addr_writes: 0,
            data_writes: 0,
            data_writes_outside_vblank: 0,
            ctrl_writes_while_rendering: 0,
            mask_writes_while_rendering: 0,
            scroll_writes_while_rendering: 0,
            addr_writes_while_rendering: 0,
            data_writes_while_rendering: 0,
            status_reads: 0,
            vblank_count: 0,
            suppress_next_vblank_status: false,
        }
    }
}

impl PpuState {
    pub fn set_nametable_mirroring(&mut self, mirroring: NametableMirroring) {
        self.nametable_mirroring = mirroring;
    }

    pub fn read_register(
        &mut self,
        reg: u16,
        frame: u64,
        cycle: u64,
        trace_cfg: TraceConfig,
        sink: &mut TraceSink,
    ) -> u8 {
        self.read_register_timed(reg, frame, cycle, 0, trace_cfg, sink)
    }

    pub fn read_register_timed(
        &mut self,
        reg: u16,
        frame: u64,
        cycle: u64,
        cpu_read_offset: u8,
        trace_cfg: TraceConfig,
        sink: &mut TraceSink,
    ) -> u8 {
        let value = match reg & 7 {
            2 => {
                self.status_reads += 1;
                // The instruction core advances devices after each instruction. A
                // status read whose data-bus cycle crosses the VBlank boundary must
                // still observe the flag before the pending NMI is serviced.
                let reads_vblank_start = self.status & 0x80 == 0
                    && self.vblank_starts_within(cpu_read_offset as u16 * 3);
                let v = if reads_vblank_start {
                    self.suppress_next_vblank_status = true;
                    self.status | 0x80
                } else {
                    self.status
                };
                self.status &= !0x80;
                self.scroll_latch = false;
                self.addr_latch = false;
                v
            }
            4 => self.oam[self.oam_addr as usize],
            7 => {
                let value = self.read_vram(self.vram_addr);
                self.increment_vram_addr();
                value
            }
            _ => 0,
        };
        if trace_cfg.ppu {
            let mut event = TraceEvent::new("ppu.reg_read", frame, cycle);
            event.addr = Some(0x2000 | (reg & 7));
            event.value = Some(value);
            event.scanline = Some(self.scanline);
            event.dot = Some(self.dot);
            sink.push(event);
        }
        value
    }

    fn vblank_starts_within(&self, ppu_ticks: u16) -> bool {
        self.scanline == 240 && ppu_ticks >= DOTS_PER_SCANLINE - self.dot
    }

    pub fn write_register(
        &mut self,
        reg: u16,
        value: u8,
        frame: u64,
        cycle: u64,
        trace_cfg: TraceConfig,
        sink: &mut TraceSink,
    ) -> Option<(u16, u8)> {
        let mut chr_write = None;
        let canonical = 0x2000 | (reg & 7);
        let visible_scanline = (0..240).contains(&self.scanline);
        let current_rendering_enabled = self.mask & 0x18 != 0;
        let resulting_rendering_enabled = if reg & 7 == 1 {
            value & 0x18 != 0
        } else {
            current_rendering_enabled
        };
        let write_during_rendering =
            visible_scanline && self.status & 0x80 == 0 && resulting_rendering_enabled;
        // Reading PPUSTATUS clears bit 7, but does not end the physical VBlank
        // interval. Using that latch produced false VRAM hazards inside NMI.
        let rendering_scanline = visible_scanline || self.scanline == 261;
        let ppudata_outside_vblank_hazard =
            reg & 7 == 7 && rendering_scanline && current_rendering_enabled;

        match reg & 7 {
            0 => {
                self.ctrl = value;
                self.temp_addr = (self.temp_addr & 0xF3FF) | (((value as u16) & 0x03) << 10);
                self.ctrl_writes += 1;
                if write_during_rendering {
                    self.ctrl_writes_while_rendering += 1;
                }
            }
            1 => {
                self.mask = value;
                self.mask_writes += 1;
                if write_during_rendering {
                    self.mask_writes_while_rendering += 1;
                }
            }
            3 => self.oam_addr = value,
            4 => {
                self.oam[self.oam_addr as usize] = value;
                self.oam_addr = self.oam_addr.wrapping_add(1);
            }
            5 => {
                self.scroll_writes += 1;
                if write_during_rendering {
                    self.scroll_writes_while_rendering += 1;
                }
                if !self.addr_latch {
                    self.fine_x = value & 0x07;
                    self.scroll_x = value;
                    self.temp_addr = (self.temp_addr & 0xFFE0) | ((value as u16) >> 3);
                    self.scroll_latch = true;
                    self.addr_latch = true;
                } else {
                    self.scroll_y = value;
                    self.temp_addr = (self.temp_addr & 0x8C1F)
                        | (((value as u16) & 0x07) << 12)
                        | (((value as u16) & 0xF8) << 2);
                    self.scroll_latch = false;
                    self.addr_latch = false;
                }
            }
            6 => {
                self.addr_writes += 1;
                if write_during_rendering {
                    self.addr_writes_while_rendering += 1;
                }
                if !self.addr_latch {
                    self.temp_addr = ((value as u16 & 0x3F) << 8) | (self.temp_addr & 0x00FF);
                    self.addr_latch = true;
                    self.scroll_latch = true;
                } else {
                    self.temp_addr = (self.temp_addr & 0xFF00) | value as u16;
                    self.vram_addr = self.temp_addr & 0x7FFF;
                    self.addr_latch = false;
                    self.scroll_latch = false;
                }
            }
            7 => {
                self.data_writes += 1;
                if ppudata_outside_vblank_hazard {
                    self.data_writes_outside_vblank += 1;
                }
                if write_during_rendering {
                    self.data_writes_while_rendering += 1;
                }
                if self.vram_addr < 0x2000 {
                    chr_write = Some((self.vram_addr, value));
                } else {
                    self.write_vram(self.vram_addr, value);
                }
                self.increment_vram_addr();
            }
            _ => {}
        }
        if trace_cfg.ppu {
            let mut event = TraceEvent::new("ppu.reg_write", frame, cycle);
            event.addr = Some(canonical);
            event.value = Some(value);
            event.scanline = Some(self.scanline);
            event.dot = Some(self.dot);
            if write_during_rendering {
                event.severity = Some("warn".to_string());
                event.message =
                    Some("PPU register write during visible rendering candidate".to_string());
            } else if ppudata_outside_vblank_hazard {
                event.severity = Some("warn".to_string());
                event.message = Some("PPUDATA write outside VBlank candidate".to_string());
            }
            sink.push(event);
        }
        chr_write
    }

    pub fn clock_cpu_cycles<F>(
        &mut self,
        cpu_cycles: u64,
        frame: u64,
        cpu_cycle: u64,
        trace_cfg: TraceConfig,
        sink: &mut TraceSink,
        mut read_pattern: F,
    ) -> bool
    where
        F: FnMut(u16, u64, u64, TraceConfig, &mut TraceSink) -> u8,
    {
        let mut nmi_requested = false;
        let mut ppu_ticks = cpu_cycles.saturating_mul(3);
        while ppu_ticks > 0 {
            let remaining_in_line = (DOTS_PER_SCANLINE - self.dot) as u64;
            let step = ppu_ticks.min(remaining_in_line);
            self.dot += step as u16;
            ppu_ticks -= step;
            if self.dot >= DOTS_PER_SCANLINE {
                self.dot = 0;
                if self.scanline >= 0 && self.scanline < 240 {
                    self.render_scanline(
                        self.scanline as usize,
                        frame,
                        cpu_cycle,
                        trace_cfg,
                        sink,
                        &mut read_pattern,
                    );
                    if self.mask & 0x18 != 0 {
                        self.increment_render_y();
                        self.copy_render_x_from_temp();
                    }
                } else if self.scanline == SCANLINES_PER_NTSC_FRAME - 1 && self.mask & 0x18 != 0 {
                    self.copy_render_y_from_temp();
                    self.copy_render_x_from_temp();
                }
                self.scanline += 1;
                if self.scanline == 241 {
                    nmi_requested |= self.begin_vblank(frame, cpu_cycle, trace_cfg, sink);
                } else if self.scanline >= SCANLINES_PER_NTSC_FRAME {
                    self.scanline = 0;
                    self.end_vblank();
                    self.rendered_frames += 1;
                    self.last_pixel_hash = self.compute_pixel_hash();
                    if trace_cfg.ppu {
                        let mut event = TraceEvent::new("ppu.frame_complete", frame, cpu_cycle);
                        event.message = Some(format!(
                            "Rendered frame {}; pixel_hash={:016X}",
                            self.rendered_frames, self.last_pixel_hash
                        ));
                        sink.push(event);
                    }
                }
            }
        }
        nmi_requested
    }

    pub fn begin_vblank(
        &mut self,
        frame: u64,
        cycle: u64,
        trace_cfg: TraceConfig,
        sink: &mut TraceSink,
    ) -> bool {
        if self.suppress_next_vblank_status {
            self.suppress_next_vblank_status = false;
        } else {
            self.status |= 0x80;
        }
        self.vblank_count += 1;
        if trace_cfg.ppu || trace_cfg.nmi {
            let mut event = TraceEvent::new("ppu.vblank_set", frame, cycle);
            event.scanline = Some(self.scanline);
            event.dot = Some(self.dot);
            event.message = Some("VBlank flag set".to_string());
            sink.push(event);
        }
        self.ctrl & 0x80 != 0
    }

    pub fn end_vblank(&mut self) {
        self.status &= !0x80;
        self.suppress_next_vblank_status = false;
    }

    pub fn current_frame_stats(&self, frame: u64) -> PpuFrameStats {
        PpuFrameStats {
            frame,
            rendered_scanlines: self.rendered_scanlines as u32,
            pattern_fetches: self.pattern_fetches,
            sprite0_hit_candidates: self.sprite0_hit_candidates,
            sprite_overflow_candidates: self.sprite_overflow_candidates,
            pixel_hash: self.last_pixel_hash,
        }
    }

    fn increment_render_y(&mut self) {
        if self.vram_addr & 0x7000 != 0x7000 {
            self.vram_addr += 0x1000;
            return;
        }

        self.vram_addr &= !0x7000;
        let mut coarse_y = (self.vram_addr & 0x03E0) >> 5;
        if coarse_y == 29 {
            coarse_y = 0;
            self.vram_addr ^= 0x0800;
        } else if coarse_y == 31 {
            coarse_y = 0;
        } else {
            coarse_y += 1;
        }
        self.vram_addr = (self.vram_addr & !0x03E0) | (coarse_y << 5);
    }

    fn copy_render_x_from_temp(&mut self) {
        self.vram_addr = (self.vram_addr & !0x041F) | (self.temp_addr & 0x041F);
    }

    fn copy_render_y_from_temp(&mut self) {
        self.vram_addr = (self.vram_addr & !0x7BE0) | (self.temp_addr & 0x7BE0);
    }

    fn render_scanline<F>(
        &mut self,
        y: usize,
        frame: u64,
        cycle: u64,
        trace_cfg: TraceConfig,
        sink: &mut TraceSink,
        read_pattern: &mut F,
    ) where
        F: FnMut(u16, u64, u64, TraceConfig, &mut TraceSink) -> u8,
    {
        let rendering_enabled = self.mask & 0x18 != 0;
        let backdrop = self.palette[0] & 0x3F;
        self.frame_buffer[y * NES_WIDTH..(y + 1) * NES_WIDTH].fill(backdrop);
        if !rendering_enabled {
            return;
        }

        self.rendered_scanlines += 1;

        let bg_enabled = self.mask & 0x08 != 0;
        let sprites_enabled = self.mask & 0x10 != 0;
        let show_left_bg = self.mask & 0x02 != 0;
        let show_left_sprites = self.mask & 0x04 != 0;
        let bg_pattern_base: u16 = if self.ctrl & 0x10 != 0 {
            0x1000
        } else {
            0x0000
        };
        let sprite_pattern_base: u16 = if self.ctrl & 0x08 != 0 {
            0x1000
        } else {
            0x0000
        };
        let sprite_height = if self.ctrl & 0x20 != 0 { 16 } else { 8 };
        let mut sprite_count = 0u8;
        let mut bg_opaque = [false; NES_WIDTH];

        if sprites_enabled {
            for i in 0..64 {
                let sy = self.oam[i * 4] as i16 + 1;
                if (y as i16) >= sy && (y as i16) < sy + sprite_height {
                    sprite_count = sprite_count.saturating_add(1);
                }
            }

            if sprite_count > 8 {
                self.status |= 0x20;
                self.sprite_overflow_candidates += 1;
            }
        }

        if bg_enabled {
            let base_x = (((self.vram_addr >> 10) & 0x01) as usize) * 256
                + ((self.vram_addr & 0x001F) as usize) * 8
                + self.fine_x as usize;
            let base_y = (((self.vram_addr >> 11) & 0x01) as usize) * 240
                + (((self.vram_addr >> 5) & 0x001F) as usize) * 8
                + ((self.vram_addr >> 12) & 0x07) as usize;
            let world_y = base_y % 480;
            let nametable_y = world_y / 240;
            let local_y = world_y % 240;
            let tile_y = local_y / 8;
            let fine_y = local_y % 8;
            let mut cached_nt_index = usize::MAX;
            let mut cached_tile_x = 0usize;
            let mut cached_nametable_base = 0x2000usize;
            let mut cached_lo = 0u8;
            let mut cached_hi = 0u8;

            for (x, opaque) in bg_opaque.iter_mut().enumerate() {
                if x < 8 && !show_left_bg {
                    continue;
                }
                let world_x = (base_x + x) % 512;
                let nametable_x = world_x / 256;
                let local_x = world_x % 256;
                let tile_x = local_x / 8;
                let fine_tile_x = local_x % 8;
                let nametable = nametable_y * 2 + nametable_x;
                let nametable_base = 0x2000 + nametable * 0x400;
                let nt_index = nametable_base + tile_y * 32 + tile_x;
                if nt_index != cached_nt_index {
                    let tile = self.read_vram(nt_index as u16);
                    let pattern_addr = bg_pattern_base + (tile as u16) * 16 + (fine_y as u16);
                    cached_lo = read_pattern(pattern_addr, frame, cycle, trace_cfg, sink);
                    cached_hi = read_pattern(pattern_addr + 8, frame, cycle, trace_cfg, sink);
                    self.pattern_fetches += 2;
                    cached_nt_index = nt_index;
                    cached_tile_x = tile_x;
                    cached_nametable_base = nametable_base;
                }

                let bits = pattern_pixel_bits(cached_lo, cached_hi, fine_tile_x);
                let idx = y * NES_WIDTH + x;
                if bits == 0 {
                    self.frame_buffer[idx] = backdrop;
                } else {
                    self.frame_buffer[idx] =
                        self.bg_color_index(cached_nametable_base, cached_tile_x, tile_y, bits);
                    *opaque = true;
                }
            }
        }

        if sprites_enabled {
            self.render_sprites_on_scanline(
                y,
                sprite_pattern_base,
                sprite_height,
                show_left_sprites,
                &bg_opaque,
                frame,
                cycle,
                trace_cfg,
                sink,
                read_pattern,
            );
            // Empty OAM slots still fetch sprite patterns. In particular MMC3
            // must see the sprite-table A12 edge on scanlines without sprites.
            // This renderer remains scanline based, rather than dot accurate.
            for _ in sprite_count.min(8)..8 {
                let address = sprite_pattern_addr(sprite_pattern_base, sprite_height, 0xFF, 0);
                read_pattern(address, frame, cycle, trace_cfg, sink);
                read_pattern(address + 8, frame, cycle, trace_cfg, sink);
                self.pattern_fetches += 2;
            }
        }

        if trace_cfg.ppu && y.is_multiple_of(32) {
            let mut event = TraceEvent::new("ppu.scanline_rendered", frame, cycle);
            event.scanline = Some(y as i16);
            event.message = Some(format!(
                "Rendered visible scanline {y}; sprite_count={sprite_count}"
            ));
            sink.push(event);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_sprites_on_scanline<F>(
        &mut self,
        y: usize,
        sprite_pattern_base: u16,
        sprite_height: i16,
        show_left_sprites: bool,
        bg_opaque: &[bool; NES_WIDTH],
        frame: u64,
        cycle: u64,
        trace_cfg: TraceConfig,
        sink: &mut TraceSink,
        read_pattern: &mut F,
    ) where
        F: FnMut(u16, u64, u64, TraceConfig, &mut TraceSink) -> u8,
    {
        // Sprite evaluation selects the FIRST eight OAM entries on this line.
        // Draw those in reverse so lower OAM indices retain pixel priority.
        let mut selected = [0usize; 8];
        let mut count = 0;
        for i in 0..64usize {
            let row = y as i16 - (self.oam[i * 4] as i16 + 1);
            if row >= 0 && row < sprite_height {
                selected[count] = i;
                count += 1;
                if count == 8 {
                    break;
                }
            }
        }
        for &sprite_index in selected[..count].iter().rev() {
            let base = sprite_index * 4;
            let y0 = self.oam[base] as i16 + 1;
            let mut row = y as i16 - y0;
            if row < 0 || row >= sprite_height {
                continue;
            }

            let tile = self.oam[base + 1];
            let attr = self.oam[base + 2];
            let x0 = self.oam[base + 3] as i16;

            if attr & 0x80 != 0 {
                row = sprite_height - 1 - row;
            }

            let pattern_addr =
                sprite_pattern_addr(sprite_pattern_base, sprite_height, tile, row as usize);
            let lo = read_pattern(pattern_addr, frame, cycle, trace_cfg, sink);
            let hi = read_pattern(pattern_addr + 8, frame, cycle, trace_cfg, sink);
            self.pattern_fetches += 2;

            for px in 0..8usize {
                let x = x0 + px as i16;
                if x < 0 || x >= NES_WIDTH as i16 {
                    continue;
                }

                let x = x as usize;
                if x < 8 && !show_left_sprites {
                    continue;
                }

                let pattern_x = if attr & 0x40 != 0 { 7 - px } else { px };
                let bits = pattern_pixel_bits(lo, hi, pattern_x);
                if bits == 0 {
                    continue;
                }

                if sprite_index == 0 && bg_opaque[x] && x != 255 {
                    self.status |= 0x40;
                    self.sprite0_hit_candidates += 1;
                }

                let behind_background = attr & 0x20 != 0;
                if behind_background && bg_opaque[x] {
                    continue;
                }

                self.frame_buffer[y * NES_WIDTH + x] = self.sprite_color_index(attr, bits);
            }
        }
    }

    fn bg_color_index(&self, nametable_base: usize, tile_x: usize, tile_y: usize, bits: u8) -> u8 {
        let attr_addr = nametable_base + 0x3C0 + (tile_y / 4) * 8 + (tile_x / 4);
        let attr = self.read_vram(attr_addr as u16);
        let shift = ((tile_y % 4) / 2) * 4 + ((tile_x % 4) / 2) * 2;
        let palette = ((attr >> shift) & 0x03) as usize;
        self.palette[palette * 4 + bits as usize] & 0x3F
    }

    fn sprite_color_index(&self, attr: u8, bits: u8) -> u8 {
        let palette = (attr & 0x03) as usize;
        self.palette[0x10 + palette * 4 + bits as usize] & 0x3F
    }

    fn read_vram(&self, addr: u16) -> u8 {
        let index = self.normalize_vram_addr(addr) as usize;
        if (0x3F00..=0x3FFF).contains(&(addr & 0x3FFF)) {
            self.palette[(index - 0x3F00) % 32]
        } else {
            self.vram[index]
        }
    }

    fn write_vram(&mut self, addr: u16, value: u8) {
        let index = self.normalize_vram_addr(addr) as usize;
        if (0x3F00..=0x3FFF).contains(&(addr & 0x3FFF)) {
            let i = (index - 0x3F00) % 32;
            self.palette[i] = value;
        } else {
            self.vram[index] = value;
        }
    }

    fn normalize_vram_addr(&self, addr: u16) -> u16 {
        let mut a = addr & 0x3FFF;
        if (0x3000..=0x3EFF).contains(&a) {
            a -= 0x1000;
        }
        if (0x2000..=0x2FFF).contains(&a) {
            let offset = a - 0x2000;
            let logical_table = offset / 0x0400;
            let physical_table = match self.nametable_mirroring {
                NametableMirroring::Vertical => logical_table & 1,
                NametableMirroring::Horizontal => logical_table >> 1,
                NametableMirroring::SingleScreenLow => 0,
                NametableMirroring::SingleScreenHigh => 1,
                NametableMirroring::FourScreen => logical_table,
            };
            return 0x2000 + physical_table * 0x0400 + (offset & 0x03FF);
        }
        if (0x3F00..=0x3FFF).contains(&a) {
            let mut palette = (a - 0x3F00) % 32;
            if matches!(palette, 0x10 | 0x14 | 0x18 | 0x1C) {
                palette -= 0x10;
            }
            return 0x3F00 + palette;
        }
        a
    }

    fn increment_vram_addr(&mut self) {
        let inc = if self.ctrl & 0x04 != 0 { 32 } else { 1 };
        self.vram_addr = self.vram_addr.wrapping_add(inc) & 0x3FFF;
    }

    fn compute_pixel_hash(&self) -> u64 {
        let mut hash = 0xcbf29ce484222325u64;
        for b in &self.frame_buffer {
            hash ^= *b as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash
    }
}

fn pattern_pixel_bits(lo: u8, hi: u8, x: usize) -> u8 {
    let shift = 7 - x;
    ((lo >> shift) & 1) | (((hi >> shift) & 1) << 1)
}

fn sprite_pattern_addr(pattern_base: u16, sprite_height: i16, tile: u8, row: usize) -> u16 {
    if sprite_height == 16 {
        let table_base = ((tile & 0x01) as u16) * 0x1000;
        let tile_index = (tile & 0xFE) as u16 + (row / 8) as u16;
        table_base + tile_index * 16 + (row % 8) as u16
    } else {
        pattern_base + (tile as u16) * 16 + row as u16
    }
}

#[cfg(test)]
mod tests {
    use super::PpuState;
    use crate::mapper::NametableMirroring;
    use crate::trace::{TraceConfig, TraceSink};

    #[test]
    fn reading_status_does_not_make_physical_vblank_vram_writes_unsafe() {
        let mut ppu = PpuState { mask: 0x1E, scanline: 246, status: 0, ..PpuState::default() };
        let mut sink=TraceSink::default();
        ppu.write_register(0x2007,0x42,0,0,TraceConfig::full(),&mut sink);
        assert!(!sink.events.iter().any(|e| e.severity.as_deref()==Some("warn")));
        ppu.scanline=100;
        ppu.write_register(0x2007,0x42,0,0,TraceConfig::full(),&mut sink);
        assert!(sink.events.iter().any(|e| e.severity.as_deref()==Some("warn")));
    }

    #[test]
    fn scroll_and_address_share_the_write_toggle_for_raster_split() {
        let mut ppu=PpuState::default();let mut sink=TraceSink::default();
        // $2006 high, $2005 Y, $2005 X, $2006 low is the raster-scroll
        // sequence. Fine Y bit 2 lives at bit 14 of the internal v register.
        for (address,value) in [(0x2006,0x08),(0x2005,238),(0x2005,0),(0x2006,0xA0)] {
            ppu.write_register(address,value,0,0,TraceConfig::none(),&mut sink);
        }
        assert_eq!(ppu.vram_addr,0x6BA0);
        assert_eq!(ppu.fine_x,0);
        assert!(!ppu.addr_latch && !ppu.scroll_latch);
    }

    #[test]
    fn sprite_evaluation_renders_only_first_eight_and_honors_oam_order() {
        let mut ppu = PpuState { mask: 0x14, ..PpuState::default() };
        ppu.oam.fill(0xF8);
        ppu.palette[0] = 0x0F;
        ppu.palette[0x11] = 0x21;
        ppu.palette[0x15] = 0x16;
        for i in 0..9 {
            ppu.oam[i * 4] = 9;
            ppu.oam[i * 4 + 1] = 1;
            ppu.oam[i * 4 + 2] = if i == 0 { 0 } else { 1 };
            ppu.oam[i * 4 + 3] = if i == 1 { 16 } else { (16 + i * 8) as u8 };
        }
        let mut sink = TraceSink::default();
        let mut pattern = |a: u16, _: u64, _: u64, _: TraceConfig, _: &mut TraceSink| {
            if a & 8 == 0 {
                0xFF
            } else {
                0
            }
        };
        ppu.render_scanline(10, 0, 0, TraceConfig::none(), &mut sink, &mut pattern);
        assert_eq!(ppu.frame_buffer[10 * 256 + 16], 0x21);
        assert_eq!(ppu.frame_buffer[10 * 256 + 80], 0x0F);
        assert_ne!(ppu.status & 0x20, 0);
    }

    #[test]
    fn empty_scanlines_still_fetch_sprite_table_for_mapper_irq() {
        let mut ppu = PpuState { mask: 0x1E, ctrl: 0x08, ..PpuState::default() };
        ppu.oam.fill(0xF8);
        let mut addresses = Vec::new();
        let mut sink = TraceSink::default();
        ppu.render_scanline(
            20,
            0,
            0,
            TraceConfig::none(),
            &mut sink,
            &mut |a, _, _, _, _| {
                addresses.push(a);
                0
            },
        );
        assert!(addresses.iter().any(|a| a & 0x1000 == 0));
        assert_eq!(addresses.iter().filter(|a| **a & 0x1000 != 0).count(), 16);
    }

    #[test]
    fn timed_status_read_observes_vblank_start_on_its_data_cycle() {
        let mut ppu = PpuState {
            ctrl: 0x80,
            status: 0x20,
            scanline: 240,
            dot: 333,
            ..PpuState::default()
        };
        let mut sink = TraceSink::default();

        let status = ppu.read_register_timed(0x2002, 0, 100, 3, TraceConfig::none(), &mut sink);

        assert_eq!(status, 0xA0);
        assert!(ppu.suppress_next_vblank_status);
        assert!(ppu.begin_vblank(0, 103, TraceConfig::none(), &mut sink));
        assert_eq!(ppu.status & 0x80, 0);
        assert!(!ppu.suppress_next_vblank_status);
    }

    #[test]
    fn timed_status_read_before_boundary_does_not_report_vblank() {
        let mut ppu = PpuState {
            status: 0x20,
            scanline: 240,
            dot: 333,
            ..PpuState::default()
        };
        let mut sink = TraceSink::default();

        let status = ppu.read_register_timed(0x2002, 0, 100, 2, TraceConfig::none(), &mut sink);

        assert_eq!(status, 0x20);
        assert!(!ppu.suppress_next_vblank_status);
    }

    #[test]
    fn pattern_table_data_write_is_returned_for_mapper_routing() {
        let mut ppu = PpuState {
            vram_addr: 0x0123,
            ..PpuState::default()
        };
        let mut sink = TraceSink::default();

        let chr_write = ppu.write_register(0x2007, 0x5A, 0, 100, TraceConfig::none(), &mut sink);

        assert_eq!(chr_write, Some((0x0123, 0x5A)));
        assert_eq!(ppu.vram[0x0123], 0);
        assert_eq!(ppu.vram_addr, 0x0124);
    }

    #[test]
    fn disabled_rendering_outputs_backdrop_instead_of_a_stale_scanline() {
        let mut ppu = PpuState::default();
        ppu.palette[0] = 0x22;
        ppu.frame_buffer.fill(0x11);
        let mut sink = TraceSink::default();
        let mut pattern =
            |_addr: u16, _frame: u64, _cycle: u64, _cfg: TraceConfig, _sink: &mut TraceSink| 0;

        ppu.render_scanline(10, 0, 0, TraceConfig::none(), &mut sink, &mut pattern);

        assert!(
            ppu.frame_buffer[10 * super::NES_WIDTH..11 * super::NES_WIDTH]
                .iter()
                .all(|pixel| *pixel == 0x22)
        );
        assert_eq!(ppu.frame_buffer[9 * super::NES_WIDTH], 0x11);
    }

    #[test]
    fn scroll_register_updates_render_coordinates_and_internal_temp_address() {
        let mut ppu = PpuState {
            mask: 0x0A,
            ..PpuState::default()
        };
        ppu.palette[1] = 1;
        ppu.palette[2] = 2;
        ppu.vram[0x2000] = 1;
        ppu.vram[0x2001] = 2;
        let mut sink = TraceSink::default();
        let mut pattern =
            |addr: u16, _frame: u64, _cycle: u64, _cfg: TraceConfig, _sink: &mut TraceSink| {
                match addr {
                    0x0010 => 0xFF,
                    0x0028 => 0xFF,
                    _ => 0,
                }
            };

        ppu.render_scanline(0, 0, 0, TraceConfig::none(), &mut sink, &mut pattern);
        assert_eq!(ppu.frame_buffer[0], 1);

        ppu.write_register(0x2000, 0x01, 0, 0, TraceConfig::none(), &mut sink);
        ppu.write_register(0x2005, 0x08, 0, 0, TraceConfig::none(), &mut sink);
        ppu.write_register(0x2005, 0x00, 0, 0, TraceConfig::none(), &mut sink);
        assert_eq!(ppu.scroll_x, 8);
        assert_eq!(ppu.scroll_y, 0);
        assert_eq!(ppu.fine_x, 0);
        assert_eq!((ppu.temp_addr >> 10) & 0x03, 0x01);

        ppu.ctrl = 0;
        ppu.temp_addr &= !0x0C00;
        ppu.vram_addr = ppu.temp_addr;
        ppu.render_scanline(0, 0, 0, TraceConfig::none(), &mut sink, &mut pattern);
        assert_eq!(ppu.frame_buffer[0], 2);
    }

    #[test]
    fn loopy_scroll_copies_temp_axes_and_wraps_vertical_position() {
        let mut ppu = PpuState {
            vram_addr: 0x7000 | (29 << 5),
            temp_addr: 0x0800 | (7 << 5) | 3,
            ..PpuState::default()
        };

        ppu.increment_render_y();
        assert_eq!(ppu.vram_addr & 0x7000, 0);
        assert_eq!(ppu.vram_addr & 0x03E0, 0);
        assert_eq!(ppu.vram_addr & 0x0800, 0x0800);

        ppu.copy_render_x_from_temp();
        assert_eq!(ppu.vram_addr & 0x041F, 3);
        ppu.copy_render_y_from_temp();
        assert_eq!(ppu.vram_addr & 0x7BE0, ppu.temp_addr & 0x7BE0);
    }

    #[test]
    fn nametable_and_palette_addresses_follow_nes_mirroring() {
        let mut ppu = PpuState::default();

        ppu.set_nametable_mirroring(NametableMirroring::Vertical);
        assert_eq!(ppu.normalize_vram_addr(0x2000), 0x2000);
        assert_eq!(ppu.normalize_vram_addr(0x2400), 0x2400);
        assert_eq!(ppu.normalize_vram_addr(0x2800), 0x2000);
        assert_eq!(ppu.normalize_vram_addr(0x2C00), 0x2400);

        ppu.set_nametable_mirroring(NametableMirroring::Horizontal);
        assert_eq!(ppu.normalize_vram_addr(0x2000), 0x2000);
        assert_eq!(ppu.normalize_vram_addr(0x2400), 0x2000);
        assert_eq!(ppu.normalize_vram_addr(0x2800), 0x2400);
        assert_eq!(ppu.normalize_vram_addr(0x2C00), 0x2400);

        ppu.set_nametable_mirroring(NametableMirroring::SingleScreenLow);
        assert_eq!(ppu.normalize_vram_addr(0x2C00), 0x2000);
        ppu.set_nametable_mirroring(NametableMirroring::SingleScreenHigh);
        assert_eq!(ppu.normalize_vram_addr(0x2000), 0x2400);
        assert_eq!(ppu.normalize_vram_addr(0x3000), 0x2400);

        assert_eq!(ppu.normalize_vram_addr(0x3F10), 0x3F00);
        assert_eq!(ppu.normalize_vram_addr(0x3F34), 0x3F04);
    }
}
