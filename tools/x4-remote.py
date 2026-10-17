"""Drives QuireInk on an Xteink X4 over USB: presses its buttons, takes
screenshots and tells what the screen shows, as the emulator's scripts do
(see shells/desktop/src/headless.rs). The firmware's side is in
shells/xteink/src/remote.rs.

Usage:
  python tools/x4-remote.py [--port COM10] [--script STEPS] [--screenshot FILE]
  python tools/x4-remote.py --log 30     (the X4's log for 30 seconds)
  python tools/x4-remote.py --put book.epub books/book.epub   (a file onto its card)
  python tools/x4-remote.py --profile --script "page-forward,wait:2000"
      (where the time goes during the steps: the functions the processor
      was in most, from the firmware's symbols; needs rustup's llvm-tools)

STEPS, separated by commas:
  page-forward       a short press of a button (back, confirm, left, right,
                     page-back, page-forward, power)
  long:confirm       a long press
  hold:confirm:2000  a press held so many milliseconds
  wait:500           500 ms pass
  screen             print the screen's id
  describe           print what the screen shows, part by part
  heap               print the heap used and free, in bytes
  refresh:full       refresh the panel from what the controller holds (also
                     refresh:fast), nothing sent
  input              print how the buttons and the battery are read
  shot:out/a.png     a screenshot now

Needs pyserial (pip install pyserial). Without --port, the X4 is found by
its USB id.
"""

import argparse
import bisect
import glob
import os
import struct
import subprocess
import sys
import time
import zlib
from pathlib import Path

import serial
from serial.tools import list_ports

# Espressif's USB serial and JTAG unit, built into the X4's ESP32-C3.
USB_ID = (0x303A, 0x1001)
# As the emulator: comfortably above the long-press threshold.
LONG_MS = 1200
# A command that takes longer has failed.
TIMEOUT_S = 60


def find_port():
    for port in list_ports.comports():
        if (port.vid, port.pid) == USB_ID:
            return port.device
    sys.exit("No X4 found on USB; name its port with --port.")


def open_port(name):
    port = serial.Serial()
    port.port = name
    port.timeout = 0.2
    # Without DTR and RTS, opening the port does not reset the chip.
    port.dtr = False
    port.rts = False
    port.open()
    return port


class X4:
    def __init__(self, port):
        self.port = port
        self.buffer = b""
        # The X4's log is printed, too.
        self.verbose = False

    def line(self, deadline):
        """The next line from the X4, or None once the deadline passed."""
        while b"\n" not in self.buffer:
            if time.monotonic() > deadline:
                return None
            self.buffer += self.port.read(self.port.in_waiting or 1)
        line, self.buffer = self.buffer.split(b"\n", 1)
        return line.decode("utf-8", "replace").rstrip("\r")

    def ask(self, command, until):
        """Sends a command and returns the X4's answers (lines with "QI:")
        up to the one starting with `until`."""
        self.port.reset_input_buffer()
        self.buffer = b""
        self.port.write(command.encode() + b"\n")
        return self.expect(command, until)

    def expect(self, command, until):
        """The X4's answers to `command` up to the one starting with `until`."""
        answers = []
        deadline = time.monotonic() + TIMEOUT_S
        while True:
            line = self.line(deadline)
            if line is None:
                sys.exit(f"{command}: no answer from the X4")
            if not line.startswith("QI:"):
                if self.verbose:
                    print(line)
                continue
            answer = line[3:]
            if answer.startswith(("ERR", "BUSY")):
                sys.exit(f"{command}: {answer}")
            answers.append(answer)
            if answer.startswith(until):
                return answers

    def put(self, local, path):
        """Copies the file `local` to `path` on the X4's card."""
        data = Path(local).read_bytes()
        self.ask(f"put {len(data)} {path}", "READY")
        started = time.monotonic()
        for at in range(0, len(data), 512):
            self.port.write(b"data " + data[at : at + 512].hex().encode() + b"\n")
        self.expect(f"put {path}", "OK")
        seconds = time.monotonic() - started
        print(f"{path}: {len(data) // 1000} KB in {seconds:.1f} s")

    def press(self, button, ms):
        self.ask(f"hold {button} {ms}", "OK")

    def screenshot(self, path):
        answers = self.ask("shot", "END")
        _, width, height, bits = answers[0].split()
        width, height, bits = int(width), int(height), int(bits)
        rows = {}
        for answer in answers[1:-1]:
            _, y, runs = answer.split(" ", 2) if answer.count(" ") >= 2 else (*answer.split(), "")
            data = bytearray()
            for i in range(0, len(runs), 4):
                data += bytes([int(runs[i + 2 : i + 4], 16)]) * int(runs[i : i + 2], 16)
            rows[int(y)] = data
        levels = (1 << bits) - 1
        pixels = bytearray()
        for y in range(height):
            data = rows.get(y, b"")
            pixels.append(0)  # PNG filter: none
            for x in range(width):
                bit = x * bits
                byte = data[bit // 8] if bit // 8 < len(data) else 0xFF
                level = (byte >> (8 - bits - bit % 8)) & levels
                pixels.append(level * 255 // levels)
        write_png(path, width, height, bytes(pixels))
        print(f"screenshot saved to {path}")


FIRMWARE = Path(__file__).resolve().parent.parent / "shells/xteink/target/riscv32imc-unknown-none-elf/release/quireink-xteink"


def symbols(elf):
    """The firmware's functions: sorted start addresses, sizes and names."""
    pattern = os.path.expanduser("~/.rustup/toolchains/*/lib/rustlib/*/bin/llvm-nm*")
    nm = next(iter(glob.glob(pattern)), "llvm-nm")
    out = subprocess.run(
        [nm, "--demangle", "--numeric-sort", "--print-size", "--defined-only", str(elf)],
        capture_output=True, text=True, check=True,
    ).stdout
    starts, sizes, names = [], [], []
    for line in out.splitlines():
        parts = line.split(" ", 3)
        if len(parts) == 4 and parts[2] in "tTwW":
            starts.append(int(parts[0], 16))
            sizes.append(int(parts[1], 16))
            # Without the hash and the generic arguments, which only differ.
            names.append(parts[3].rsplit("::h", 1)[0])
    return starts, sizes, names


def rom_functions(x4):
    """Where the ROM's functions lie: its table of jumps (as the linker
    scripts of esp-rom-sys name them) read from the chip and followed."""
    table = {}
    pattern = str(FIRMWARE.parent / "build/esp-rom-sys-*/out/rom/*.ld")
    for path in glob.glob(pattern):
        for line in Path(path).read_text(errors="replace").splitlines():
            name, _, value = line.partition(" = ")
            value = value.strip().rstrip(";")
            if value.startswith("0x4000") and name.strip().isidentifier():
                table[int(value, 16)] = name.strip()
    if not table:
        return [], []
    low, high = min(table), max(table)
    words = {}
    for at in range(low, high + 4, 4096 * 4):
        count = min(4096, (high + 4 - at) // 4)
        for answer in x4.ask(f"peek {at:x} {count}", "END")[:-1]:
            fields = answer.split()
            base = int(fields[1], 16)
            for i, word in enumerate(fields[2:]):
                words[base + i * 4] = int(word, 16)
    targets = []
    for entry, name in table.items():
        insn = words.get(entry)
        # A jump (jal): where it goes.
        if insn is None or insn & 0x7F != 0x6F:
            continue
        imm = ((insn >> 31) & 1) << 20 | ((insn >> 12) & 0xFF) << 12
        imm |= ((insn >> 20) & 1) << 11 | ((insn >> 21) & 0x3FF) << 1
        if imm & (1 << 20):
            imm -= 1 << 21
        targets.append((entry + imm, name))
    targets.sort()
    return [t for t, _ in targets], [n for _, n in targets]


def report(places, elf, x4, top=30):
    starts, sizes, names = symbols(elf)
    rom_starts, rom_names = rom_functions(x4) if any(0x4004_0000 <= p < 0x4006_0000 for p in places) else ([], [])
    counts = {}
    for pc in places:
        i = bisect.bisect_right(starts, pc) - 1
        if i >= 0 and pc < starts[i] + max(sizes[i], 1):
            name = names[i]
        elif 0x4000_0000 <= pc < 0x4006_0000:
            # The chip's ROM: its C library, soft floats, …
            j = bisect.bisect_right(rom_starts, pc) - 1
            name = f"ROM {rom_names[j]}" if j >= 0 else "ROM"
        else:
            name = f"? {pc:x}"
        counts[name] = counts.get(name, 0) + 1
    # Waiting (the main loop's rest, the panel's refresh) is not work.
    idle = sum(n for name, n in counts.items() if "Delay>::delay" in name)
    busy = (len(places) - idle) or 1
    print(f"{len(places)} samples, {len(places) * 5 / 1000:.1f} s, of them waiting {idle * 5 / 1000:.1f} s")
    crates = {}
    for name, n in counts.items():
        if "Delay>::delay" not in name:
            # The crate: the first path in the name, past `<` and `&`.
            first = name.lstrip("<&").split("::")[0] if "::" in name else name
            crates[first] = crates.get(first, 0) + n
    print("by crate, of the work:")
    for name, n in sorted(crates.items(), key=lambda kv: -kv[1])[:12]:
        print(f"{n * 100 / busy:5.1f} %  {name}")
    print("by function, of the work:")
    for name, n in sorted(counts.items(), key=lambda kv: -kv[1])[:top]:
        if "Delay>::delay" not in name:
            print(f"{n * 100 / busy:5.1f} %  {name[:140]}")


def write_png(path, width, height, gray_rows):
    """A gray PNG, from rows each starting with a filter byte."""

    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    header = struct.pack(">IIBBBBB", width, height, 8, 0, 0, 0, 0)
    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header)
    png += chunk(b"IDAT", zlib.compress(gray_rows, 9)) + chunk(b"IEND", b"")
    Path(path).parent.mkdir(parents=True, exist_ok=True)
    Path(path).write_bytes(png)


def run(x4, step):
    parts = step.split(":")
    match parts:
        case ["wait", ms]:
            time.sleep(int(ms) / 1000)
        case ["long", button]:
            x4.press(button, LONG_MS)
        case ["hold", button, ms]:
            x4.press(button, int(ms))
        case ["screen"]:
            print(x4.ask("screen", "SCREEN")[-1].split(" ", 1)[-1])
        case ["input"]:
            print(x4.ask("input", "INPUT")[-1])
        case ["refresh", kind]:
            print(x4.ask(f"refresh {kind}", "OK")[-1])
        case ["heap"]:
            _, used, free = x4.ask("heap", "HEAP")[-1].split()
            print(f"heap: {used} bytes used, {free} free")
        case ["describe"]:
            for answer in x4.ask("describe", "END")[:-1]:
                _, role, x, y, w, h, state, *label = answer.split(" ")
                state = "" if state == "-" else f" ({state.replace('+', ', ')})"
                print(f'{role} "{" ".join(label)}"{state} at {x},{y} {w}x{h}')
        case ["shot", path]:
            x4.screenshot(path)
        case [button]:
            x4.press(button, 60)
        case _:
            sys.exit(f"unknown step: {step}")


def main():
    # Labels come in UTF-8, whatever the console's code page.
    sys.stdout.reconfigure(encoding="utf-8")
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--port")
    parser.add_argument("--script", default="")
    parser.add_argument("--screenshot")
    parser.add_argument("--log", type=float, metavar="SECONDS")
    parser.add_argument("-v", "--verbose", action="store_true", help="print the X4's log too")
    parser.add_argument("--profile", action="store_true", help="where the time goes during the steps")
    parser.add_argument("--trace", action="store_true", help="what the steps do on the storage")
    parser.add_argument("--put", nargs=2, action="append", default=[], metavar=("FILE", "PATH"))
    args = parser.parse_args()
    port = open_port(args.port or find_port())
    x4 = X4(port)
    x4.verbose = args.verbose
    if args.log:
        deadline = time.monotonic() + args.log
        while (line := x4.line(deadline)) is not None:
            print(line)
        return
    for local, path in args.put:
        x4.put(local, path)
    if args.profile:
        x4.ask("profile start", "OK")
    if args.trace:
        x4.ask("trace start", "OK")
    for step in filter(None, (s.strip() for s in args.script.split(","))):
        run(x4, step)
    if args.trace:
        for answer in x4.ask("trace stop", "END")[:-1]:
            print(answer[3:])
    if args.profile:
        answers = x4.ask("profile stop", "END")
        places = [int(pc, 16) for a in answers[:-1] for pc in a.split()[1:]]
        report(places, FIRMWARE, x4)
    if args.screenshot:
        x4.screenshot(args.screenshot)


if __name__ == "__main__":
    main()
