"""Check clipboard requests from the four select-*.json scripts.

Run against the fixture from scripts/make-fixture-repo.sh at 1180x720, scale 2,
with --chrome and --trace <gestures|horizontal|wrap|virtual>.jsonl. Also capture
multiline-dark.json with --tree at mark:multiline in select-gestures.json, and
virtual-dark.json at 3.34 seconds in select-virtual.json.
"""

import json
from pathlib import Path
import sys


def trace(directory, name):
    return [json.loads(line) for line in (directory / f"{name}.jsonl").read_text().splitlines()]


def copies(frames):
    return [json.loads(event.removeprefix("diff-copy "))
            for frame in frames for event in frame.get("ev", ())
            if event.startswith("diff-copy ")]


def code(directory, tree, file):
    nodes = json.loads((directory / f"{tree}.json").read_text())["elements"]
    return {int(node["path"].split(f"/line/{file}/")[1].split("/")[0]): node["text"]
            for node in nodes if node.get("key") == "code" and f"/line/{file}/" in node["path"]}


def main(directory):
    retired = "/// How long to wait before trying a request again."
    replacement = "/// How long to wait before trying a request again, with jitter so clients spread out."
    picked = "    base: Duration,\n    max: Duration,\n    attemp"
    displayed = code(directory, "multiline-dark", 8)
    assert len(displayed) == 40, "expected all of src/lib.rs's projected rows in tree overscan"
    assert copies(trace(directory, "gestures")) == [
        "\n".join([retired[4:], replacement, "pub struct Backoff {", "    base: Duration,",
                   "    max: Duration,", "    attempt: u32,", "    jitter: f32,"]),
        "Backoff",
        "    base: Duration,",
        picked,
        picked,  # native menu's scripted Copy action
        "\n".join(displayed[i] for i in sorted(displayed)),
    ], "drag, word, line, shift-click, menu and Cmd+A"

    assert copies(trace(directory, "horizontal")) == [
        replacement[18:] + "\npub struct Backoff {",
    ], "horizontal hit-testing"
    assert copies(trace(directory, "wrap")) == [
        replacement[16:78],
        replacement[16:] + "\npub struct Backoff {\n    base: Dur",
    ], "wrapped hit-testing and soft line breaks"

    frames = trace(directory, "virtual")
    offsets = {frame["t"]: max(entry[2] for entry in frame["sc"])
               for frame in frames if frame.get("sc")}
    assert offsets[2700] - offsets[2150] > 400, "holding at the edge should scroll without input"
    before, large, after = copies(frames)
    generated = "\n".join([
        "onst VALUE_0099: u32 = 99;",
        "pub const VALUE_0100: u32 = 100;",
        "pub const VALUE_0100: u32 = 1000; // bumped",
        *(f"pub const VALUE_{i:04d}: u32 = {i};" for i in range(101, 610)),
        "pub const VALUE_0610:",
    ])
    assert before == generated[:generated.index("pub const VALUE_0116:")] + "pub const VALUE_0116:"
    assert large == after == generated, "copy before and after scrolling away"
    mounted = code(directory, "virtual-dark", 7)
    assert 98 not in mounted and 350 not in mounted and 610 in mounted, "anchor and middle unmounted"
    print("Selection traces passed: gestures, horizontal scroll, Wrap, edge scroll, and 513 rows across virtual mounts.")


if __name__ == "__main__":
    main(Path(sys.argv[1]))
