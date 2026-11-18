"""Install Git Viewer using the pinned Rune installer and bundle transaction."""

import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
from types import SimpleNamespace

NAME = "Git Viewer"
BUNDLE_ID = "local.git-viewer"
EXE = "git-viewer-rune"
ROOT = Path(__file__).resolve().parent.parent


def manual_path(bin_dir):
    return bin_dir.parent / "share/man/man1/git-review.1"


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
    with tempfile.TemporaryDirectory(prefix="git-viewer-ready-") as directory:
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


def install_prefixed(rune_scripts, bundle, exe, binary, icon):
    # The upstream CLI fixes its destination at ~/Applications. Use the same
    # staging, signing, swap and rollback code with an explicit destination.
    sys.dont_write_bytecode = True
    sys.path.insert(0, str(rune_scripts))
    import mac_app_install as installer

    flags = SimpleNamespace(name=NAME, id=BUNDLE_ID, exe=EXE, bin=binary, icon=icon,
                            bundle=None, scheme=None, mic=None)
    with installer.Bundle(bundle) as transaction:
        installer.build(transaction.stage, flags)
        old = installer.running(exe)
        if old:
            args = [installer.process_args(pid) for pid in old]
            if any(value != args[0] for value in args) or installer.running(exe) != old:
                raise RuntimeError("running Git Viewer instances changed or have different arguments")
            transaction.replace(
                lambda: installer.quit_apps(exe, old),
                lambda _: launch_direct(installer, exe, args[0]),
                lambda: installer.quit_apps(exe, installer.running(exe)),
            )
        else:
            transaction.publish()
            transaction.committed = True


def install(app_dir, bin_dir):
    rune_scripts, binary = cargo_build()
    bundle = app_dir / f"{NAME}.app"
    executable = bundle / "Contents/MacOS" / EXE
    icon = ROOT / "assets/icon.icns"
    template = (ROOT / "scripts/git-review").read_text()
    launcher = template.replace(
        'app_executable="$HOME/Applications/Git Viewer.app/Contents/MacOS/git-viewer-rune"',
        f"app_executable={shlex.quote(str(executable))}",
        1,
    )
    if launcher == template:
        raise RuntimeError("couldn’t set the installed git-review app path")
    bin_dir.mkdir(parents=True, exist_ok=True)
    manual = manual_path(bin_dir)
    manual.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(mode="w", dir=bin_dir, delete=False) as file:
        staged = Path(file.name)
        file.write(launcher)
    try:
        staged.chmod(0o755)
        # The default uses Rune's CLI, including its running-app readiness and rollback.
        if app_dir == Path.home() / "Applications":
            subprocess.run([
                rune_scripts / "mac-app", "--name", NAME, "--id", BUNDLE_ID, "--exe", EXE,
                "--bin", binary, "--icon", icon, "--only-relaunch",
            ], check=True)
        else:
            install_prefixed(rune_scripts, bundle, executable, binary, icon)
            print(bundle)
        staged.replace(bin_dir / "git-review")
        shutil.copyfile(ROOT / "scripts/git-review.1", manual)
    finally:
        staged.unlink(missing_ok=True)
    print(bin_dir / "git-review")


def uninstall(app_dir, bin_dir):
    (bin_dir / "git-review").unlink(missing_ok=True)
    manual_path(bin_dir).unlink(missing_ok=True)
    bundle = app_dir / f"{NAME}.app"
    if bundle.is_symlink():
        bundle.unlink()
    elif bundle.exists():
        shutil.rmtree(bundle)


def main(action):
    if sys.platform != "darwin":
        raise RuntimeError("the Git Viewer bundle targets macOS")
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
        sys.exit(f"Git Viewer: {error}")
