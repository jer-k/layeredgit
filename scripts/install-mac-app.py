"""Install layeredgit using the pinned Rune installer and bundle transaction."""

import json
import os
from pathlib import Path
import plistlib
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
from types import SimpleNamespace
from xml.parsers.expat import ExpatError

NAME = "layeredgit"
BUNDLE_ID = "local.layeredgit"
EXE = NAME
LEGACY_BUNDLE = "Git Viewer.app"
LEGACY_ID = "local.git-viewer"
LEGACY_EXE = "git-viewer-rune"
LEGACY_COMMAND = "git-review"
ROOT = Path(__file__).resolve().parent.parent


def manual_path(bin_dir, command=NAME):
    return bin_dir.parent / "share/man/man1" / f"{command}.1"


def cargo_build():
    target = subprocess.check_output(["rustc", "--print", "host-tuple"], text=True).strip()
    metadata = json.loads(subprocess.check_output([
        "cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", target,
    ]))
    resolve = metadata["resolve"]
    root = next(node for node in resolve["nodes"] if node["id"] == resolve["root"])
    rune_id = next(dep["pkg"] for dep in root["deps"] if dep["name"] == "rune")
    rune_manifest = next(p["manifest_path"] for p in metadata["packages"] if p["id"] == rune_id)
    rune_scripts = Path(rune_manifest).parents[2] / "scripts"
    command = ["cargo", "build", "--release", "--locked", "--no-default-features",
               "--package", EXE, "--message-format=json-render-diagnostics"]
    binary = None
    with subprocess.Popen(command, stdout=subprocess.PIPE, text=True) as build:
        for line in build.stdout:
            message = json.loads(line)
            if message["reason"] == "compiler-message":
                sys.stderr.write(message["message"].get("rendered") or "")
            if message["reason"] == "compiler-artifact" and message["target"]["name"] == EXE:
                binary = message.get("executable")
        if build.wait() != 0 or not binary:
            raise RuntimeError("release build failed")
    return rune_scripts, Path(binary)


def launch_direct(installer, exe, args):
    """Relaunch a prefixed test or portable bundle without Launch Services."""
    with tempfile.TemporaryDirectory(prefix="layeredgit-ready-") as directory:
        env = {**os.environ, "RUNE_READY_DIR": directory}
        process = subprocess.Popen([exe, *args], env=env, start_new_session=True,
                                   stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                   stderr=subprocess.DEVNULL)
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            if process.poll() is not None:
                raise RuntimeError(f"replacement exited with status {process.returncode}")
            if installer.receipt_ready(Path(directory), BUNDLE_ID, process.pid):
                return
            time.sleep(0.1)
        raise RuntimeError("replacement did not become ready within 15 seconds")


def rune_installer(rune_scripts):
    sys.dont_write_bytecode = True
    sys.path.insert(0, str(rune_scripts))
    import mac_app_install as installer

    return installer


def install_prefixed(installer, bundle, exe, binary, icon):
    # The upstream CLI fixes its destination at ~/Applications. Use the same
    # staging, signing, swap and rollback code with an explicit destination.
    flags = SimpleNamespace(name=NAME, id=BUNDLE_ID, exe=EXE, bin=binary, icon=icon,
                            bundle=None, scheme=None, mic=None)
    with installer.Bundle(bundle) as transaction:
        installer.build(transaction.stage, flags)
        old = installer.running(exe)
        if old:
            args = [installer.process_args(pid) for pid in old]
            if any(value != args[0] for value in args) or installer.running(exe) != old:
                raise RuntimeError(f"running {NAME} instances changed or have different arguments")
            transaction.replace(
                lambda: installer.quit_apps(exe, old),
                lambda _: launch_direct(installer, exe, args[0]),
                lambda: installer.quit_apps(exe, installer.running(exe)),
            )
        else:
            transaction.publish()
            transaction.committed = True


def is_legacy_bundle(bundle):
    contents = bundle / "Contents"
    info = contents / "Info.plist"
    if any(path.is_symlink() for path in (bundle, contents, info)) or not info.is_file():
        return False
    try:
        with info.open("rb") as file:
            metadata = plistlib.load(file)
        return isinstance(metadata, dict) and metadata.get("CFBundleIdentifier") == LEGACY_ID
    except (FileNotFoundError, plistlib.InvalidFileException, ValueError, ExpatError):
        return False


def legacy_lines(path):
    if path.is_symlink() or not path.is_file():
        return []
    return path.read_bytes().splitlines()


def clean_legacy(app_dir, bin_dir, installer=None):
    bundle = app_dir / LEGACY_BUNDLE
    if is_legacy_bundle(bundle):
        if installer is not None:
            exe = bundle / "Contents/MacOS" / LEGACY_EXE
            installer.quit_apps(exe, installer.running(exe))
        if is_legacy_bundle(bundle):
            shutil.rmtree(bundle)

    launcher = bin_dir / LEGACY_COMMAND
    lines = legacy_lines(launcher)
    launcher_path = f"/{LEGACY_BUNDLE}/Contents/MacOS/{LEGACY_EXE}".encode()
    if (
        b"# Installed by just install with the executable path for this installation." in lines
        and b"usage='Usage: git review [BASE]" in lines
        and any(line.startswith(b"app_executable=") and launcher_path in line for line in lines)
    ):
        launcher.unlink()

    manual = manual_path(bin_dir, LEGACY_COMMAND)
    if legacy_lines(manual)[:6] == [
        b".TH GIT-REVIEW 1",
        b".SH NAME",
        rb"git-review \- open Git Viewer for the current working tree",
        b".SH SYNOPSIS",
        b".B git review",
        b".RI [ BASE ]",
    ]:
        manual.unlink()


def install(app_dir, bin_dir):
    rune_scripts, binary = cargo_build()
    bundle = app_dir / f"{NAME}.app"
    executable = bundle / "Contents/MacOS" / EXE
    icon = ROOT / "assets/icon.icns"
    template = (ROOT / "scripts" / NAME).read_text()
    launcher = template.replace(
        f'app_executable="$HOME/Applications/{NAME}.app/Contents/MacOS/{EXE}"',
        f"app_executable={shlex.quote(str(executable))}",
        1,
    )
    if launcher == template:
        raise RuntimeError(f"couldn’t set the installed {NAME} app path")
    bin_dir.mkdir(parents=True, exist_ok=True)
    manual = manual_path(bin_dir)
    manual.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(mode="w", dir=bin_dir, delete=False) as file:
        staged = Path(file.name)
        file.write(launcher)
    try:
        installer = rune_installer(rune_scripts)
        staged.chmod(0o755)
        # The default uses Rune's CLI, including its running-app readiness and rollback.
        if app_dir == Path.home() / "Applications":
            subprocess.run([
                rune_scripts / "mac-app", "--name", NAME, "--id", BUNDLE_ID, "--exe", EXE,
                "--bin", binary, "--icon", icon, "--only-relaunch",
            ], check=True)
        else:
            install_prefixed(installer, bundle, executable, binary, icon)
            print(bundle)
        staged.replace(bin_dir / NAME)
        shutil.copyfile(ROOT / "scripts" / f"{NAME}.1", manual)
        clean_legacy(app_dir, bin_dir, installer)
    finally:
        staged.unlink(missing_ok=True)
    print(bin_dir / NAME)


def uninstall(app_dir, bin_dir):
    clean_legacy(app_dir, bin_dir)
    (bin_dir / NAME).unlink(missing_ok=True)
    manual_path(bin_dir).unlink(missing_ok=True)
    bundle = app_dir / f"{NAME}.app"
    if bundle.is_symlink():
        bundle.unlink()
    elif bundle.exists():
        shutil.rmtree(bundle)


def main(action):
    if sys.platform != "darwin":
        raise RuntimeError(f"the {NAME} bundle targets macOS")
    home = Path.home()
    app_dir = Path(os.environ.get("APP_DIR") or home / "Applications").expanduser().resolve()
    bin_dir = Path(os.environ.get("BIN_DIR") or home / ".local/bin").expanduser().resolve()
    if action == "install":
        install(app_dir, bin_dir)
    elif action == "uninstall":
        uninstall(app_dir, bin_dir)
    else:
        raise RuntimeError(f"unknown installer action: {action}")


if __name__ == "__main__":
    try:
        main(sys.argv[1])
    except (OSError, RuntimeError, subprocess.CalledProcessError) as error:
        sys.exit(f"{NAME}: {error}")
