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
u.GetWindowThreadProcessId.argtypes = [w.HWND, c.POINTER(w.DWORD)]
WM_CLOSE = 0x0010


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


if __name__ == '__main__':
    run_close_question()
