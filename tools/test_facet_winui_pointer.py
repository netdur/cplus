"""Real pointer checks; build facet_agent_winui_smoke first."""
import ctypes as c
from ctypes import wintypes as w
import http.client
import os
import re
import subprocess
import time
from test_facet_winui_parity import OUT, u, wait_for
from test_agent_winui_http import connect, rpc

u.GetDpiForWindow.argtypes = [w.HWND]
u.GetDpiForWindow.restype = w.UINT


def move_cursor(x, y):
    # Inject movement through the input queue, including remote desktop sessions.
    u.mouse_event(0x8001, round(x * 65535 / (u.GetSystemMetrics(0) - 1)),
                  round(y * 65535 / (u.GetSystemMetrics(1) - 1)), 0, 0)


def run():
    runtime = OUT / 'runtime'
    log = OUT / 'pointer.log'
    conn = None
    with log.open('w', encoding='utf-8') as output:
        process = subprocess.Popen([str(runtime / 'facet_agent_winui_smoke.exe')],
            cwd=runtime, env={**os.environ, 'CPLUS_FACET_POINTER': '1'},
            stdout=output, stderr=subprocess.STDOUT, creationflags=subprocess.CREATE_NO_WINDOW)
        try:
            content = wait_for(log, 'POINTER READY', process)
            conn = connect(int(re.search(r'POINTER READY (\d+)', content)[1]))
            hwnd = u.FindWindowW(None, 'Facet WinUI pointer')
            assert hwnd
            assert u.SetWindowPos(hwnd, None, 50, 50, 1100, 800, 0x0040)
            u.SetForegroundWindow(hwnd)
            time.sleep(.3)
            assert u.GetForegroundWindow() == hwnd
            origin = w.POINT(0, 0)
            assert u.ClientToScreen(hwnd, c.byref(origin))
            for key in ('button', 'icon', 'text'):
                row = next(n for n in rpc(conn, 'describe_ui', {'mode': 'full'}) if n['id'] == key)
                f = row['frame']
                move_cursor(origin.x + round(f['x'] + f['w']/2), origin.y + round(f['y'] + f['h']/2))
                time.sleep(.1)
                u.mouse_event(2, 0, 0, 0, 0)
                if key != 'text':
                    wait_for(log, 'PRESS ' + key, process, 5)
                u.mouse_event(4, 0, 0, 0, 0)
                if key != 'text':
                    wait_for(log, 'RELEASE ' + key, process, 5)
                wait_for(log, 'CLICK ' + key, process, 5)

            # TogglePattern does not raise native Click; Facet still delivers
            # the same activation callback and state as a real mouse click.
            for key in ('button', 'icon', 'text'):
                assert rpc(conn, 'click', {'id': key})['outcome'] == 'allowed'

            row = next(n for n in rpc(conn, 'describe_ui', {'mode': 'full'}) if n['id'] == 'slider')
            f = row['frame']
            sx, sy = origin.x + round(f['x'] + f['w']/2), origin.y + round(f['y'] + f['h']/2)
            # Recoloring at drag start must wait until release; a second drag
            # verifies that subscriptions follow the replacement native thumb.
            for attempt, start_x, end_x in ((1, sx, sx + 70), (2, sx + 70, sx)):
                move_cursor(start_x, sy)
                time.sleep(.15)
                u.mouse_event(2, 0, 0, 0, 0)
                wait_for(log, f'SLIDER START {attempt}', process, 5)
                move_cursor(end_x, sy)
                time.sleep(.25)
                u.mouse_event(4, 0, 0, 0, 0)
                wait_for(log, f'SLIDER END {attempt}', process, 5)
                time.sleep(.3)

            # Canvas has no automation peer; its Facet layout frame is in DIPs.
            x, y, width, height = map(float, re.search(r'CANVAS FRAME (\S+) (\S+) (\S+) (\S+)', content).groups())
            scale = u.GetDpiForWindow(hwnd) / 96
            cx = origin.x + round((x + width/2) * scale)
            cy = origin.y + round((y + height/2) * scale)
            move_cursor(cx, cy)
            wait_for(log, 'CANVAS HOVER', process, 5)
            move_cursor(cx + 10, cy)
            wait_for(log, 'CANVAS MOVE', process, 5)
            # Right-click is not a primary drawing interaction.
            u.mouse_event(8, 0, 0, 0, 0); u.mouse_event(16, 0, 0, 0, 0)
            time.sleep(.2)
            assert 'CANVAS PRESS' not in log.read_text(encoding='utf-8')
            u.mouse_event(2, 0, 0, 0, 0)
            wait_for(log, 'CANVAS PRESS 1', process, 5)
            move_cursor(origin.x + round((x + width + 40) * scale), cy)
            wait_for(log, 'CANVAS DRAG', process, 5)
            u.mouse_event(4, 0, 0, 0, 0)
            wait_for(log, 'CANVAS RELEASE', process, 5)
            wait_for(log, 'CANVAS LEAVE', process, 5)
            assert 'CANVAS CANCEL' not in log.read_text(encoding='utf-8')

            move_cursor(cx, cy)
            u.mouse_event(2, 0, 0, 0, 0)
            wait_for(log, 'CANVAS PRESS 2', process, 5)
            assert rpc(conn, 'click', {'id': 'cancel'})['outcome'] == 'allowed'
            wait_for(log, 'CANVAS CANCEL', process, 5)
            u.mouse_event(4, 0, 0, 0, 0)
            time.sleep(.55)
            u.mouse_event(2, 0, 0, 0, 0)
            wait_for(log, 'PASS: removed canvas while pressed', process, 5)
            u.mouse_event(4, 0, 0, 0, 0)
            # Closing the app inside Invoke can close its HTTP connection first.
            try:
                rpc(conn, 'click', {'id': 'finish'})
            except (ConnectionError, http.client.RemoteDisconnected):
                pass
            assert process.wait(timeout=10) == 0, log.read_text(encoding='utf-8')
            assert 'PASS: pointer teardown' in log.read_text(encoding='utf-8')
            print(log.read_text(encoding='utf-8'))
        finally:
            u.mouse_event(4, 0, 0, 0, 0)
            if conn:
                conn.close()
            if process.poll() is None:
                process.kill(); process.wait()


if __name__ == '__main__':
    run()
