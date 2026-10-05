"""Native state checks plus real keyboard submission and local WebView navigation."""
import base64
import ctypes as c
from ctypes import wintypes as w
import os
from pathlib import Path
import re
import subprocess
import time
import sys
from test_agent_winui_http import connect, rpc

# Native probes include Unicode text; redirected Windows stdout may use cp1252.
sys.stdout.reconfigure(encoding="utf-8")
sys.stderr.reconfigure(encoding="utf-8")

ROOT = Path(__file__).resolve().parents[1]
EXAMPLE = ROOT / 'examples/facet_agent_winui_smoke'
OUT = EXAMPLE / 'out'
u = c.WinDLL('user32', use_last_error=True)
u.SetProcessDpiAwarenessContext(c.c_void_p(-4))
u.FindWindowW.restype = w.HWND
u.GetForegroundWindow.restype = w.HWND
u.SetForegroundWindow.argtypes = [w.HWND]
u.ClientToScreen.argtypes = [w.HWND, c.POINTER(w.POINT)]
u.SetWindowPos.argtypes = [w.HWND, w.HWND, c.c_int, c.c_int, c.c_int, c.c_int, w.UINT]
u.WindowFromPoint.argtypes = [w.POINT]
u.WindowFromPoint.restype = w.HWND
u.GetAncestor.argtypes = [w.HWND, w.UINT]
u.GetAncestor.restype = w.HWND
u.GetCursorPos.argtypes = [c.POINTER(w.POINT)]


def wait_for(log, marker, process, seconds=30):
    deadline = time.monotonic() + seconds
    while True:
        contents = log.read_text(encoding='utf-8', errors='replace')
        if marker in contents:
            return contents
        assert process.poll() is None, contents
        assert time.monotonic() < deadline, contents
        time.sleep(.05)


def run():
    env = {**os.environ, 'CPLUS_FACET_PARITY': '1'}
    for name in ('first', 'second', 'third'):
        page = OUT / (name + '.html')
        page.write_text(f'<html><body><h1>{name}</h1></body></html>', encoding='utf-8')
        env['CPLUS_PARITY_' + name.upper()] = page.as_uri()
    runtime = OUT / 'runtime'
    log = OUT / 'parity.log'
    with log.open('w', encoding='utf-8') as output:
        process = subprocess.Popen([str(runtime / 'facet_agent_winui_smoke.exe')],
            cwd=runtime, env=env, stdout=output, stderr=subprocess.STDOUT,
            creationflags=subprocess.CREATE_NO_WINDOW)
        conn = None
        try:
            content = wait_for(log, 'PARITY INPUT READY', process)
            conn = connect(int(re.search(r'PARITY INPUT READY (\d+)', content)[1]))
            wait_for(log, 'PASS: browser history', process)
            hwnd = u.FindWindowW(None, 'Facet WinUI parity')
            assert hwnd
            assert u.SetWindowPos(hwnd, None, 50, 50, 1100, 800, 0x0040)
            u.SetForegroundWindow(hwnd)
            time.sleep(.3)
            assert u.GetForegroundWindow() == hwnd, 'Parity window must be foreground for desktop input'
            origin = w.POINT(0, 0)
            assert u.ClientToScreen(hwnd, c.byref(origin))
            for key, marker in [('list-row-0', 'LIST TAP 1'), ('list-row-0', 'LIST TAP 2'),
                                ('list-row-1', 'LIST TAP 3'),
                                ('grid-row-0', 'GRID SELECT 1'), ('grid-row-1', 'GRID SELECT 2'),
                                ('grid-row-0', 'GRID SELECT 3')]:
                tree = rpc(conn, 'describe_ui', {'mode': 'full'})
                row = next(n for n in tree if n['id'] == key)
                frame = row['frame']
                assert frame['w'] > 0 and frame['h'] > 0, row
                point = w.POINT(origin.x + round(frame['x'] + frame['w']/2),
                                origin.y + round(frame['y'] + frame['h']/2))
                assert u.GetAncestor(u.WindowFromPoint(point), 2) == hwnd, (key, frame, 'Window obscured')
                # Queue movement before the click, as the pointer probe does;
                # cursor warping alone is unreliable in remote desktop sessions.
                u.mouse_event(0x8001, round(point.x * 65535 / (u.GetSystemMetrics(0) - 1)),
                              round(point.y * 65535 / (u.GetSystemMetrics(1) - 1)), 0, 0)
                time.sleep(.1)
                cursor = w.POINT()
                assert u.GetCursorPos(c.byref(cursor))
                assert abs(cursor.x - point.x) <= 1 and abs(cursor.y - point.y) <= 1, (key, point.x, point.y, cursor.x, cursor.y)
                if key.startswith('grid'):
                    u.keybd_event(17, 0, 0, 0)
                try:
                    u.mouse_event(2, 0, 0, 0, 0)
                    time.sleep(.05)  # Allow WinUI to process the pressed state before release.
                    u.mouse_event(4, 0, 0, 0, 0)
                    wait_for(log, marker, process, 5)
                finally:
                    if key.startswith('grid'):
                        u.keybd_event(17, 0, 2, 0)
                time.sleep(.55)  # Distinct taps, not a Windows double-tap gesture.
            for key in ('input', 'search', 'area'):
                assert rpc(conn, 'scroll_to', {'id': key})['outcome'] == 'allowed'
                time.sleep(.2)
                if key == 'area':
                    # Enter alone must remain a newline, not submit.
                    u.keybd_event(13, 0, 0, 0); u.keybd_event(13, 0, 2, 0)
                    time.sleep(.15)
                    assert 'SUBMIT area' not in log.read_text(encoding='utf-8')
                    u.keybd_event(17, 0, 0, 0)
                u.keybd_event(13, 0, 0, 0); u.keybd_event(13, 0, 2, 0)
                if key == 'area':
                    u.keybd_event(17, 0, 2, 0)
                wait_for(log, 'SUBMIT ' + key, process, 5)
            assert process.wait(timeout=30) == 0, log.read_text(encoding='utf-8')
            assert 'PASS: parity teardown' in log.read_text(encoding='utf-8')
            print(log.read_text(encoding='utf-8'))
        finally:
            if conn:
                conn.close()
            if process.poll() is None:
                process.kill(); process.wait()


def run_styles():
    runtime = OUT / 'runtime'
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_STYLES': '1'}, capture_output=True,
        text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: style/event teardown' in result.stdout


def run_text():
    runtime = OUT / 'runtime'
    animation = OUT / 'animated.gif'
    animation.write_bytes(base64.b64decode('R0lGODlhAgACAIEAAP8AAAAAAAAAAAAAACH/C05FVFNDQVBFMi4wAwEAAAAh+QQACgAAACwAAAAAAgACAAAIBgABCAQQEAAh+QQACgAAACwAAAAAAgACAIEAAP8AAAAAAAAAAAAIBgABCAQQEAA7'))
    fixture = OUT / 'pixel.png'
    fixture.write_bytes(base64.b64decode('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aR3sAAAAASUVORK5CYII='))
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_TEXT': '1', 'CPLUS_TEST_IMAGE': str(fixture), 'CPLUS_TEST_ANIMATION': str(animation)},
        capture_output=True, text=True, encoding='utf-8', timeout=30,
        creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: text/content teardown' in result.stdout


def run_window_buttons():
    runtime = OUT / 'runtime'
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_WINDOW_BUTTONS': '1'}, capture_output=True,
        text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: window buttons teardown' in result.stdout


def run_html_labels():
    runtime = OUT / 'runtime'
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_HTML_LABELS': '1'}, capture_output=True,
        text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: HTML labels teardown' in result.stdout


def run_carousel_sizing():
    runtime = OUT / 'runtime'
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_CAROUSEL_SIZING': '1'}, capture_output=True,
        text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: carousel sizing teardown' in result.stdout


def run_split_roles():
    runtime = OUT / 'runtime'
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_SPLIT_ROLES': '1'}, capture_output=True,
        text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: split roles teardown' in result.stdout


def run_button_breaks():
    runtime = OUT / 'runtime'
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_BUTTON_BREAKS': '1'}, capture_output=True,
        text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: button breaks teardown' in result.stdout


def run_return_keys():
    runtime = OUT / 'runtime'
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_RETURN_KEYS': '1'}, capture_output=True,
        text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: return keys teardown' in result.stdout


def run_input_visuals():
    runtime = OUT / 'runtime'
    fixture = OUT / 'input-visuals.png'
    fixture.write_bytes(base64.b64decode('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aR3sAAAAASUVORK5CYII='))
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_INPUT_VISUALS': '1', 'CPLUS_TEST_IMAGE': str(fixture)},
        capture_output=True, text=True, encoding='utf-8', timeout=30,
        creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: input visuals teardown' in result.stdout


def run_lifecycle():
    runtime = OUT / 'runtime'
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_LIFECYCLE': '1'}, capture_output=True,
        text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: list lifecycle teardown' in result.stdout


def run_actions():
    runtime = OUT / 'runtime'
    fixture = OUT / 'pixel.png'
    fixture.write_bytes(base64.b64decode('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aR3sAAAAASUVORK5CYII='))
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_ACTIONS': '1', 'CPLUS_TEST_IMAGE': str(fixture)},
        capture_output=True, text=True, encoding='utf-8', timeout=30,
        creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: action teardown' in result.stdout


def run_date_formats():
    runtime = OUT / 'runtime'
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_DATE_FORMATS': '1'}, capture_output=True,
        text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: date format teardown' in result.stdout


def run_refresh():
    runtime = OUT / 'runtime'
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_REFRESH': '1'}, capture_output=True,
        text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: refresh teardown' in result.stdout


def run_hybrid():
    for identity in ('a', 'b'):
        directory = OUT / ('hybrid-' + identity)
        directory.mkdir(parents=True, exist_ok=True)
        (directory / 'style.css').write_text('body { background-color: rgb(20, 30, 40); }', encoding='utf-8')
        for page in ('index', 'alternate'):
            (directory / (page + '.html')).write_text(
                '<!doctype html><meta charset="utf-8"><link rel="stylesheet" href="style.css">'
                f'<body data-page="{page}"><h1>Hybrid {identity.upper()}</h1>'
                '<script src="bridge%20%CE%B1.js"></script></body>', encoding='utf-8')
        (directory / 'bridge α.js').write_text(
            "window.facet.onmessage = body => window.facet.postMessage('echo:' + body);\n"
            "window.addEventListener('load', async () => {\n"
            "if (getComputedStyle(document.body).backgroundColor !== 'rgb(20, 30, 40)') throw Error('CSS missing');\n"
            f"window.facet.postMessage('ready:{identity.upper()}:' + document.body.dataset.page);\n"
            "window.facet.postMessage('missing:' + (await fetch('missing.txt')).status);\n"
            "window.facet.postMessage('override:' + (await fetch('override.txt')).status);\n"
            "});", encoding='utf-8')
    for removal in ('', 'START', 'READY', 'RESOURCE', 'MESSAGE'):
        environment = {**os.environ, 'CPLUS_FACET_HYBRID': '1'}
        if removal:
            environment['CPLUS_HYBRID_REMOVE_' + removal] = '1'
        result = subprocess.run([str(OUT / 'runtime/facet_agent_winui_smoke.exe')], cwd=EXAMPLE,
            env=environment, capture_output=True, text=True, encoding='utf-8', timeout=40,
            creationflags=subprocess.CREATE_NO_WINDOW)
        print(result.stdout, flush=True)
        assert result.returncode == 0, f"hybrid removal={removal}, exit={result.returncode}\n{result.stdout}{result.stderr}"
        assert 'PASS: hybrid callback removal and teardown' in result.stdout


def run_reordering():
    runtime = OUT / 'runtime'
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_REORDERING': '1'}, capture_output=True,
        text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: reorder teardown' in result.stdout


def run_grouped_items():
    runtime = OUT / 'runtime'
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_GROUPED_ITEMS': '1'}, capture_output=True,
        text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: grouped items teardown' in result.stdout


def run_list_refresh():
    runtime = OUT / 'runtime'
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_LIST_REFRESH': '1'}, capture_output=True,
        text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: list refresh teardown' in result.stdout


def run_time_open():
    runtime = OUT / 'runtime'
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_TIME_OPEN': '1'}, capture_output=True,
        text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: time flyout open-window teardown' in result.stdout


def run_secure_modes():
    runtime = OUT / 'runtime'
    for extra in ({}, {'CPLUS_SECURE_CLOSE_PASSWORD': '1'}):
        result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
            env={**os.environ, 'CPLUS_FACET_SECURE_MODES': '1', **extra}, capture_output=True,
            text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
        print(result.stdout)
        assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
        assert 'pending-mode teardown' in result.stdout


def run_button_modes():
    runtime = OUT / 'runtime'
    for extra in ({}, {'CPLUS_BUTTON_ROOT': '1'}, {'CPLUS_BUTTON_CLOSE_TOGGLE': '1'}):
        result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
            env={**os.environ, 'CPLUS_FACET_BUTTON_MODES': '1', **extra}, capture_output=True,
            text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
        print(result.stdout)
        assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
        assert 'pending-mode teardown' in result.stdout


def run_splits():
    runtime = OUT / 'runtime'
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_SPLITS': '1'}, capture_output=True,
        text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: split teardown' in result.stdout


def run_layout():
    runtime = OUT / 'runtime'
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_LAYOUT': '1'}, capture_output=True,
        text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: layout teardown' in result.stdout


def run_choice_colors():
    runtime = OUT / 'runtime'
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_CHOICE_COLORS': '1'}, capture_output=True,
        text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: choice color teardown' in result.stdout


def run_canvas_redraw():
    runtime = OUT / 'runtime'
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_CANVAS_REDRAW': '1'}, capture_output=True,
        text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: canvas redraw teardown' in result.stdout


def run_borders():
    runtime = OUT / 'runtime'
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_BORDERS': '1'}, capture_output=True,
        text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: border teardown' in result.stdout


def run_clear_button():
    runtime = OUT / 'runtime'
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_CLEAR_BUTTON': '1'}, capture_output=True,
        text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: clear-button teardown' in result.stdout


def run_popup_caption():
    runtime = OUT / 'runtime'
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_POPUP_CAPTION': '1'}, capture_output=True,
        text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: popup caption teardown' in result.stdout


def run_input_transform():
    runtime = OUT / 'runtime'
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_INPUT_TRANSFORM': '1'}, capture_output=True,
        text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: input casing readback and teardown' in result.stdout


def run_row_retention():
    runtime = OUT / 'runtime'
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_ROW_RETENTION': '1'}, capture_output=True,
        text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: row retention teardown' in result.stdout


def run_tree_rows():
    runtime = OUT / 'runtime'
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_TREE_ROWS': '1'}, capture_output=True,
        text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: tree row teardown' in result.stdout


def run_menus():
    runtime = OUT / 'runtime'
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_MENUS': '1'}, capture_output=True,
        text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: menu teardown' in result.stdout


def run_commands():
    runtime = OUT / 'runtime'
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_COMMANDS': '1'}, capture_output=True,
        text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: command strip teardown' in result.stdout


def run_tables():
    runtime = OUT / 'runtime'
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_TABLES': '1'}, capture_output=True,
        text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: table teardown' in result.stdout


def run_tabs():
    runtime = OUT / 'runtime'
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_TABS': '1'}, capture_output=True,
        text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: tabs teardown' in result.stdout


def run_paging():
    runtime = OUT / 'runtime'
    for extra in ({}, {'CPLUS_FACET_PAGING_CURRENT': '1'}):
        result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
            env={**os.environ, 'CPLUS_FACET_PAGING': '1', **extra}, capture_output=True,
            text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
        print(result.stdout)
        assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
        assert 'PASS: carousel teardown' in result.stdout


def run_swiping():
    runtime = OUT / 'runtime'
    for removal in ('', 'START', 'CHANGE', 'OPEN', 'CLOSE', 'END'):
        environment = {**os.environ, 'CPLUS_FACET_SWIPING': '1'}
        if removal:
            environment['CPLUS_SWIPE_REMOVE_' + removal] = '1'
        result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
            env=environment, capture_output=True, text=True, encoding='utf-8',
            timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
        print(result.stdout)
        assert result.returncode == 0, f"{removal}: exit={result.returncode}\n{result.stdout}{result.stderr}"
        assert 'PASS: swipe teardown' in result.stdout


if __name__ == '__main__':
    if '--swiping-only' in sys.argv:
        run_swiping()
        sys.exit(0)
    if '--hybrid-only' in sys.argv:
        run_hybrid()
        sys.exit(0)
    if '--reordering-only' in sys.argv:
        run_reordering()
        sys.exit(0)
    if '--grouped-items-only' in sys.argv:
        run_grouped_items()
        sys.exit(0)
    if '--list-refresh-only' in sys.argv:
        run_list_refresh()
        sys.exit(0)
    if '--time-open-only' in sys.argv:
        run_time_open()
        sys.exit(0)
    if '--secure-modes-only' in sys.argv:
        run_secure_modes()
        sys.exit(0)
    if '--button-modes-only' in sys.argv:
        run_button_modes()
        sys.exit(0)
    if '--splits-only' in sys.argv:
        run_splits()
        sys.exit(0)
    if '--clear-button-only' in sys.argv:
        run_clear_button()
        sys.exit(0)
    if '--popup-caption-only' in sys.argv:
        run_popup_caption()
        sys.exit(0)
    if '--input-transform-only' in sys.argv:
        run_input_transform()
        sys.exit(0)
    if '--row-retention-only' in sys.argv:
        run_row_retention()
        sys.exit(0)
    if '--tree-rows-only' in sys.argv:
        run_tree_rows()
        sys.exit(0)
    if '--menus-only' in sys.argv:
        run_menus()
        sys.exit(0)
    if '--window-buttons-only' in sys.argv:
        run_window_buttons()
        sys.exit(0)
    if '--html-labels-only' in sys.argv:
        run_html_labels()
        sys.exit(0)
    if '--carousel-sizing-only' in sys.argv:
        run_carousel_sizing()
        sys.exit(0)
    if '--split-roles-only' in sys.argv:
        run_split_roles()
        sys.exit(0)
    if '--return-keys-only' in sys.argv:
        run_return_keys()
        sys.exit(0)
    if '--button-breaks-only' in sys.argv:
        run_button_breaks()
        sys.exit(0)
    if '--commands-only' in sys.argv:
        run_commands()
        sys.exit(0)
    if '--paging-only' in sys.argv:
        run_paging()
        sys.exit(0)
    if '--tabs-only' in sys.argv:
        run_tabs()
        sys.exit(0)
    if '--borders-only' in sys.argv:
        run_borders()
        sys.exit(0)
    if '--canvas-redraw-only' in sys.argv:
        run_canvas_redraw()
        sys.exit(0)
    if '--choice-colors-only' in sys.argv:
        run_choice_colors()
        sys.exit(0)
    if '--layout-only' in sys.argv:
        run_layout()
        sys.exit(0)
    if '--refresh-only' in sys.argv:
        run_refresh()
        sys.exit(0)
    if '--date-formats-only' in sys.argv:
        run_date_formats()
        sys.exit(0)
    if '--actions-only' in sys.argv:
        run_actions()
        sys.exit(0)
    if '--lifecycle-only' in sys.argv:
        run_lifecycle()
        sys.exit(0)
    if '--tables-only' in sys.argv:
        run_tables()
        sys.exit(0)
    if '--input-visuals-only' in sys.argv:
        run_input_visuals()
        sys.exit(0)
    if '--styles-only' not in sys.argv and '--text-only' not in sys.argv:
        run()
    if '--text-only' not in sys.argv:
        run_styles()
    run_text()
    run_input_visuals()
    if len(sys.argv) == 1:
        run_lifecycle()
        run_actions()
        run_date_formats()
        run_refresh()
        run_layout()
        run_splits()
        run_choice_colors()
        run_canvas_redraw()
        run_borders()

        run_tables()
        run_tabs()

        run_paging()

        run_menus()

        run_commands()
        run_window_buttons()
        run_html_labels()
        run_carousel_sizing()
        run_split_roles()
        run_return_keys()

        run_button_breaks()

        run_tree_rows()

        run_row_retention()

        run_input_transform()
        run_popup_caption()
        run_clear_button()

    if len(sys.argv) == 1:
        run_button_modes()

    if len(sys.argv) == 1:
        run_secure_modes()

    if len(sys.argv) == 1:
        run_time_open()

    if len(sys.argv) == 1:
        run_list_refresh()

    if len(sys.argv) == 1:
        run_grouped_items()

    if len(sys.argv) == 1:
        run_reordering()
        run_hybrid()
        run_swiping()
