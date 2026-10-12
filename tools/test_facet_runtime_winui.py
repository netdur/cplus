"""facet_runtime on WinUI, driven from outside; build examples/facet_runtime_winui first.

A close that the user starts is a WM_CLOSE from outside, the same request a
caption button or Alt+F4 makes, so that is what these checks post.
"""
import ctypes as c
from ctypes import wintypes as w
import os
from pathlib import Path
import subprocess
import time

REPO = Path(__file__).resolve().parents[1]
RUNTIME = REPO / 'examples/facet_runtime_winui/out/runtime'
EXE = RUNTIME / 'facet_runtime_winui.exe'
u = c.WinDLL('user32', use_last_error=True)
u.FindWindowW.argtypes = [w.LPCWSTR, w.LPCWSTR]
u.FindWindowW.restype = w.HWND
u.PostMessageW.argtypes = [w.HWND, w.UINT, w.WPARAM, w.LPARAM]
u.IsWindow.argtypes = [w.HWND]
u.GetForegroundWindow.restype = w.HWND
u.GetWindowThreadProcessId.argtypes = [w.HWND, c.POINTER(w.DWORD)]
WM_CLOSE = 0x0010
UIA = REPO / 'tools/winui_uia.ps1'


def uia(action, hwnd, key):
    result = subprocess.run(['powershell.exe', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', str(UIA),
                             action, str(hwnd), key], capture_output=True, text=True, timeout=60)
    assert result.returncode == 0, result.stderr
    return result.stdout.strip()


def shows(hwnd, key, seconds=10):
    end = time.time() + seconds
    while time.time() < end:
        if uia('find', hwnd, key) == '1':
            return True
        time.sleep(.2)
    return False


def read(log):
    return log.read_text(encoding='utf-8', errors='replace')


def wait_for(log, marker, process, seconds=30):
    end = time.time() + seconds
    while time.time() < end:
        content = read(log)
        if marker in content:
            return content
        if process.poll() is not None:
            raise AssertionError(f'exited {process.returncode} before {marker!r}:\n{content}')
        time.sleep(.05)
    raise AssertionError(f'timed out waiting for {marker!r}:\n{read(log)}')


def window(title, process, seconds=15):
    end = time.time() + seconds
    while time.time() < end:
        hwnd = u.FindWindowW(None, title)
        if hwnd:
            pid = w.DWORD()
            u.GetWindowThreadProcessId(hwnd, c.byref(pid))
            if pid.value == process.pid:
                return hwnd
        time.sleep(.05)
    raise AssertionError(f'no window titled {title!r}')


def launch(log, probe):
    env = {**os.environ, 'CPLUS_RUNTIME_WINUI_PROBE': probe}
    output = log.open('w', encoding='utf-8')
    process = subprocess.Popen([str(EXE)], cwd=RUNTIME, env=env, stdout=output,
                               stderr=subprocess.STDOUT, creationflags=subprocess.CREATE_NO_WINDOW)
    return process, output


def run_close_question():
    """Closing the last window asks on_should_quit; another window does not."""
    log = RUNTIME / 'close_question.log'
    process, output = launch(log, '1')
    try:
        wait_for(log, 'READY: facet_winui', process)
        settings = window('Settings', process)
        main = window('WinUI runtime', process)

        # Not the last window: it closes and the app is not asked.
        assert u.PostMessageW(settings, WM_CLOSE, 0, 0)
        end = time.time() + 10
        while u.IsWindow(settings) and time.time() < end:
            time.sleep(.05)
        assert not u.IsWindow(settings), 'settings did not close'
        time.sleep(.5)
        assert 'PROBE should_quit' not in read(log), 'a non-last window asked on_should_quit'

        # The last window: asked, refused, still up and still running.
        assert u.PostMessageW(main, WM_CLOSE, 0, 0)
        wait_for(log, 'PROBE should_quit 1', process, 10)
        time.sleep(.5)
        assert u.IsWindow(main), 'the refused close still closed the window'
        assert process.poll() is None, 'the refused close ended the run'
        assert 'PROBE quit' not in read(log)

        # Asked again; allowed this time, the run ends cleanly.
        assert u.PostMessageW(main, WM_CLOSE, 0, 0)
        wait_for(log, 'PROBE should_quit 2', process, 10)
        assert process.wait(15) == 0, read(log)
        content = read(log)
        assert 'PROBE quit' in content
        assert content.count('PROBE should_quit') == 2, content
        assert 'outlived the run' not in content, content
    finally:
        if process.poll() is None:
            process.kill()
        output.close()
    print('PASS: close question (non-last window not asked, refusal holds, allow ends the run)')


def activate(hwnd, key):
    assert uia('focus', hwnd, key) == 'focused'
    end = time.time() + 5
    while u.GetForegroundWindow() != hwnd and time.time() < end:
        time.sleep(.05)
    assert u.GetForegroundWindow() == hwnd, f'could not activate {key}'
    time.sleep(.3)


def run_alert_window():
    """An alert opens over the window whose control asked, not the active one."""
    log = RUNTIME / 'alert_window.log'
    process, output = launch(log, '1')
    try:
        wait_for(log, 'READY: facet_winui', process)
        settings = window('Settings', process)
        main = window('WinUI runtime', process)
        assert shows(settings, 'settings:alert-later')
        # Settings asks for an alert 1.5 s from now (the press activates it);
        # main is made the active window before it fires.
        assert uia('invoke', settings, 'settings:alert-later') == 'invoked'
        activate(main, 'home:prompt')
        assert shows(settings, 'alert:title'), 'the alert is not on the window that asked'
        assert uia('find', main, 'alert:title') == '0', 'the alert went to the active window'
        assert uia('invoke', settings, 'alert:primary') == 'invoked'
        end = time.time() + 10
        while uia('find', settings, 'alert:title') == '1' and time.time() < end:
            time.sleep(.2)
        assert uia('find', settings, 'alert:title') == '0', 'the alert did not answer'
        # An immediate alert from a press goes to the pressed window.
        activate(main, 'home:prompt')
        assert uia('invoke', settings, 'settings:alert') == 'invoked'
        assert shows(settings, 'alert:title')
        assert uia('find', main, 'alert:title') == '0'
        assert uia('invoke', settings, 'alert:primary') == 'invoked'
        # NEGATIVE: the main window's own button keeps it on main.
        assert uia('invoke', main, 'home:alert') == 'invoked'
        assert shows(main, 'alert:title')
        assert uia('find', settings, 'alert:title') == '0'
    finally:
        if process.poll() is None:
            process.kill()
        output.close()
    print('PASS: alert window (a deferred alert stays on the window that asked; each window keeps its own)')


def run_worker_hop():
    """A job's worker hands its result back through run_on_main."""
    log = RUNTIME / 'worker_hop.log'
    process, output = launch(log, '0')
    try:
        wait_for(log, 'READY: facet_winui', process)
        main = window('WinUI runtime', process)
        assert shows(main, 'home:load')
        # Twice: the press writes "Loading...", so the second answer is a new hop.
        for round in (1, 2):
            assert uia('invoke', main, 'home:load') == 'invoked'
            end = time.time() + 10
            name = ''
            while time.time() < end:
                name = uia('name', main, 'home:loaded')
                if name.startswith('Loaded'):
                    break
                time.sleep(.2)
            assert name == 'Loaded 42 off the UI thread', f'round {round}: {name!r}\n{read(log)}'
            assert process.poll() is None, read(log)
    finally:
        if process.poll() is None:
            process.kill()
        output.close()
    print('PASS: worker hop (run_job runs off the UI thread and applies on it, twice)')


if __name__ == '__main__':
    run_close_question()
    run_alert_window()
    run_worker_hop()
