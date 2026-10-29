# N163, NSF and NSFe music

The CLI and shared core support mapper-19 N163 audio and standalone NSF1/NSFe
music. All examples below use one-based song numbers.

```powershell
.\kurosaki.exe nsf-info music.nsfe
.\kurosaki.exe nsf-export music.nsf --song 1 --seconds 30 --wav song.wav --json song.json
.\kurosaki.exe nsf-export music.nsfe --song 2 --wav song2.wav
.\kurosaki.exe nsfe-export music.nsf --out music.nsfe
.\kurosaki.exe nsfe-export old.nsf --out old.nsfe --song 1 --seconds 85.191
.\kurosaki.exe audio-export game.nes --frames 600 --wav game.wav --json game.json
```

`nsf-info` reports format, songs, starting song, metadata, banks and chips.
`nsf-export` executes INIT/PLAY and writes mono PCM16. The default rate is
44100 Hz; `--sample-rate` accepts 8000–192000 Hz. `--seconds` overrides metadata.
Otherwise, the selected track's declared time plus fade is used, or 30 seconds
when there is no duration. Export is bounded to 600 seconds. Fade metadata is
preserved and included in the duration; the host does not apply a gain envelope.
The JSON report binds the output to its PCM hash, peak, sample count, PLAY calls
and DPCM fetches. The CLI exports files without opening an audio device.

`nsfe-export` preserves music code/data, banks, track labels, time/fade metadata
and author fields. `--song`, `--seconds` and `--fade-seconds` can set the duration
for one selected track. Nondefault NTSC playback periods are stored in RATE.
Unknown optional chunks and playlist ordering are not preserved.

Supported playback includes NTSC/dual-region NSF1 and NSFe, unbanked or 4 KiB
banked images, 2A03/DPCM, N163, VRC6, VRC7 and FDS. N163 implements shared
128-byte waveform/register RAM, 1–8 voices, CPU-clocked phase, auto-increment
wrap, IRQ, PRG banking and restorable mapper state. NSF uses the average voice
mix to reduce DAC-switching aliases; ROM execution retains the multiplexed DAC.
The N163 digital gain is 180 PCM units per DAC unit at the core's APU scale.
Analog volume profiles and console filters are not calibrated.

PAL-only playback, NSF2 execution extensions, MMC5/Sunsoft 5B/VT02 audio and
non-returning INIT/PLAY routines produce errors. Music data is limited to 1 MiB,
with bounded metadata. NSF snapshots are not exposed. Existing VRC7/FDS
synthesis approximations remain. Mapper 19 is still classified as scaffold:
CHR/CIRAM special modes, ROM-backed nametables and battery-backed sound RAM
are incomplete. This does not establish compatibility with every mapper-19 game.

The tests generate original waveforms and 6502 programs in memory. They cover
1–8 voices, 440 Hz pitch at two rates, bank switching, DPCM/expansion mixing,
metadata round trips, malformed input and incremental rendering. They do not
establish physical-hardware or analog/listening equivalence.

## 日本語

`nsf-info` で曲情報を表示し、`nsf-export` でNSF／NSFeをWAVに出力できます。
曲番号は1始まりです。`--seconds` を省略すると曲長とfade情報の合計を使い、
曲長がなければ30秒になります。fade情報は保存・尺計算に使いますが、ホスト側の
音量フェード処理は行いません。`nsfe-export` は曲データと曲長を保ってNSFeへ変換します。
N163は1～8音に対応し、ROM実行・NSF再生の両方で使用します。
PAL専用、NSF2実行拡張、未対応音源にはエラーを返します。

## References

- [NESdev N163 audio](https://www.nesdev.org/wiki/Namco_163_audio)
- [NESdev mapper 19](https://www.nesdev.org/wiki/Namco_163)
- [NESdev NSF/NSFe format](https://www.nesdev.org/wiki/NSF)
