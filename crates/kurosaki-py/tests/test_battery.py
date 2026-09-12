"""Run with the freshly built kurosaki extension on PYTHONPATH; synthetic only."""
import tempfile
import unittest
from pathlib import Path
import kurosaki


class BatteryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="kurosaki-battery-python-")
        self.root = Path(self.temp.name)
        rom = bytearray(16 + 16384)
        rom[:4] = b"NES\x1a"
        rom[4] = 1
        rom[6] = 2
        rom[16:22] = bytes([0xee, 0x00, 0x60, 0x4c, 0x03, 0x80])
        for vector in [0x3ffa, 0x3ffc, 0x3ffe]:
            rom[16+vector:18+vector] = bytes([0, 0x80])
        self.rom = self.root / "synthetic.nes"
        self.rom.write_bytes(rom)

    def tearDown(self):
        self.temp.cleanup()

    def test_battery_cold_boot_roundtrip_and_backup(self):
        emulator = kurosaki.Emulator.from_rom(str(self.rom))
        emulator.step_frame()
        self.assertEqual(emulator.battery_ram()[0], 1)
        save = self.root / "synthetic.sav"
        emulator.save_battery_file(str(save))
        self.assertEqual(len(save.read_bytes()), 8192)
        reopened = kurosaki.Emulator.from_rom(str(self.rom), battery_path=str(save))
        self.assertEqual(reopened.battery_ram()[0], 1)
        reopened.step_frame()
        reopened.save_battery_file(str(save))
        self.assertEqual(save.read_bytes()[0], 2)
        self.assertEqual((self.root / "synthetic.sav.bak").read_bytes()[0], 1)
        emulator.load_battery_file(str(save))
        self.assertEqual(emulator.battery_ram()[0], 2)

    def test_bad_size_does_not_change_state(self):
        emulator = kurosaki.Emulator.from_rom(str(self.rom))
        before = emulator.state_hash()
        save = self.root / "bad.sav"
        save.write_bytes(b"short")
        with self.assertRaises(RuntimeError):
            emulator.load_battery_file(str(save))
        self.assertEqual(emulator.state_hash(), before)
        self.assertEqual(save.read_bytes(), b"short")
        with self.assertRaises(RuntimeError):
            kurosaki.Emulator.from_rom(str(self.rom), battery_path=str(save))

    def test_export_cannot_overwrite_loaded_rom(self):
        emulator = kurosaki.Emulator.from_rom(str(self.rom))
        before = self.rom.read_bytes()
        with self.assertRaises(RuntimeError):
            emulator.save_battery_file(str(self.rom))
        self.assertEqual(self.rom.read_bytes(), before)


if __name__ == "__main__":
    unittest.main()
