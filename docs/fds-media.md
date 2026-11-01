# FDS read channel (2026-11-02)

Loading `.fds` now starts at the **BIOS reset vector**. PRG/CHR RAM is not
preloaded with game files. The BIOS programs the disk controller and receives
the reconstructed block stream through `$4031`. The former direct-boot shortcut
is disabled for normal loading.

Supported media is the standard 65500-byte-per-side FDS layout, with or without
the 16-byte FDS header. The drive reconstructs sync, gaps and CRC for blocks
1–4, models spin-up/byte latching, transfer IRQ, independent timer IRQ,
readiness, mirroring and disk/audio gates. Repeated `$4031` reads do not advance
the drive. The stream progresses with CPU cycles.

Supply your own 8192-byte BIOS through `KUROSAKI_DISKSYS_ROM` or the documented
`disksys.rom` lookup paths. Firmware is not included in the distribution.

```powershell
$env:KUROSAKI_DISKSYS_ROM='C:\path\to\disksys.rom'
kurosaki run game.fds --fds-side 0 --frames 1200 --png boot.png
```

`--fds-side` is zero-based. The Python `Emulator.insert_fds_side(index)` method
switches media and accepts `None` to eject. Disk-side changes do not reset the CPU; wait for the game's requested disk-change sequence.

New snapshots preserve the drive's byte position, countdown, latch, IRQs, RAM
and audio state. Restore checks the attached BIOS/media identity. Old FDS
snapshots from the direct-boot implementation are incompatible; reopen the
original disk and save a new state.

FDS snapshots currently embed the attached firmware and reconstructed media.
Keep these snapshots and FDS capture bundles private; they are not included in
the distributable update package.

Media is **read-only**. `$4024` write requests do not change the input file and
are surfaced as `FDS_MEDIA_WRITE_PROTECTED` diagnostics when an active drive
requests writing. CRC failures map to `FDS_MEDIA_CRC_ERROR`. SARAKURA's FC
catalog recognizes both with media-specific guidance. Ordinary transfer trace
events do not produce generic error floods.

The timing model currently uses nominal 150 CPU cycles per byte and a fixed
spin-up delay. QD images, protected/nonstandard block layouts, physical drive
timing exactness and persistent disk writes are not covered. Successful BIOS
boot of the original KITAQ BLOCKS FC disk is verified in KUROSAKI; it does not
establish compatibility with every commercial disk or physical hardware.

Register/format references: [FDS disk format](https://www.nesdev.org/wiki/FDS_disk_format),
[FDS hardware](https://www.nesdev.org/wiki/Family_Computer_Disk_System) and
[FDS file format](https://www.nesdev.org/wiki/FDS_file_format).
