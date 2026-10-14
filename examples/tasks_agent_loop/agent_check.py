#!/usr/bin/env python3
"""Drive the RUNNING tasks_agent_loop app through its agent surface and check
its behaviour. The regression case for the loop in README.md.

    ./target/debug/tasks_agent_loop &      # the app prints where it serves
    python3 agent_check.py                 # finds it by its descriptor

Every check reads the result back from the live UI. A tool answering
"allowed" is not evidence that anything happened; the label it changed is.
Exit status is the number of failed requirements.
"""
import glob, json, os, sys, time, urllib.request

APP = "tasks_agent_loop"


def find_endpoint():
    """The newest descriptor of a live process serving APP."""
    found = []
    for path in glob.glob(f"/tmp/cplus-{os.getuid()}/mcp-{APP}-*.json"):
        pid = int(path.rsplit("-", 1)[1].split(".")[0])
        try:
            os.kill(pid, 0)
        except OSError:
            continue
        with open(path) as f:
            d = json.load(f)
        found.append((os.path.getmtime(path), d))
    if not found:
        sys.exit(f"no running {APP} found under /tmp/cplus-{os.getuid()}/")
    d = max(found, key=lambda x: x[0])[1]
    for k in ("address", "url", "http"):
        if isinstance(d.get(k), str) and d[k].startswith("http"):
            return d[k]
    for v in d.values():
        if isinstance(v, str) and v.startswith("http"):
            return v
    sys.exit(f"descriptor has no http address: {d}")


URL = None
_id = 0


def rpc(method, params=None):
    global _id
    _id += 1
    body = json.dumps({"jsonrpc": "2.0", "id": _id, "method": method, "params": params or {}}).encode()
    req = urllib.request.Request(URL, body, {"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=5) as r:
        d = json.loads(r.read())
    if "error" in d:
        raise RuntimeError(f"{method}: {d['error']}")
    return d["result"]


def nodes():
    return {n["id"]: n for n in rpc("describe_ui")}


def text_of(node_id):
    n = nodes().get(node_id)
    return None if n is None else n["text"]


def rows():
    """The task rows on screen, in order."""
    ns = nodes()
    out = []
    i = 0
    while f"task:{i}" in ns:
        out.append(ns[f"task:{i}"]["text"])
        i += 1
    return out


def type_into(node_id, value):
    v = rpc("read_text", {"id": node_id})
    params = {"id": node_id, "value": value}
    if isinstance(v, dict) and "version" in v:
        params["base_version"] = v["version"]
    return rpc("set_text", params)


def click(node_id):
    return rpc("click", {"id": node_id})


def wait_for(pred, seconds=3.0, step=0.05):
    """Bounded wait on an OBSERVATION, never on a sleep alone."""
    end = time.time() + seconds
    while time.time() < end:
        if pred():
            return True
        time.sleep(step)
    return pred()


RESULTS = []


def check(name, ok, detail=""):
    RESULTS.append((name, ok))
    print(f"{'PASS' if ok else 'FAIL'}  {name}" + (f"  — {detail}" if detail and not ok else ""))


def main():
    global URL
    URL = find_endpoint()
    info = rpc("initialize", {"clientInfo": {"name": "agent_check"}})
    check("reached the app we meant", info["serverInfo"]["name"] == APP, str(info["serverInfo"]))

    ns = nodes()
    for key, role in [("new_title", "input"), ("add", "button"), ("status", "text"),
                      ("count", "text"), ("sync", "button")]:
        check(f"discover {key} ({role})", key in ns and ns[key]["role"] == role, str(ns.get(key)))

    # R2 — a real title is added, reported, counted, and the field clears.
    before = len(rows())
    type_into("new_title", "Buy milk")
    out = click("add")
    check("add answers allowed", out.get("outcome") == "allowed", str(out))
    check("add: status says what was added", text_of("status") == "Added: Buy milk", text_of("status"))
    check("add: the row is on screen", rows()[-1:] == ["Buy milk"], str(rows()))
    check("add: the count follows", text_of("count") == f"{before + 1} task" + ("" if before + 1 == 1 else "s"), text_of("count"))
    check("add: the field is cleared", text_of("new_title") == "", repr(text_of("new_title")))

    # R3 — empty is refused and nothing is added.
    n = len(rows())
    type_into("new_title", "")
    click("add")
    check("empty title: refused", text_of("status") == "Title required", text_of("status"))
    check("empty title: nothing added", len(rows()) == n, str(rows()))

    # R4 — whitespace only is the same as empty.
    type_into("new_title", "   ")
    click("add")
    check("blank title: refused", text_of("status") == "Title required", text_of("status"))
    check("blank title: nothing added", len(rows()) == n, str(rows()))

    # R5 — surrounding whitespace is not part of a title.
    type_into("new_title", "  Call Bob  ")
    click("add")
    check("padded title: stored trimmed", rows()[-1:] == ["Call Bob"], str(rows()))
    check("padded title: reported trimmed", text_of("status") == "Added: Call Bob", repr(text_of("status")))

    # R6 — the async sync: pending first, then the result, within a bound.
    total = len(rows())
    click("sync")
    pending = text_of("status") == "Syncing…"
    disabled = not nodes()["sync"]["actionable"]
    check("sync: pending is visible", pending, text_of("status"))
    check("sync: the button is disabled while pending", disabled, str(nodes()["sync"]))
    done = wait_for(lambda: text_of("status") == f"Synced {total} tasks", seconds=3.0)
    check("sync: completes within 3s", done, text_of("status"))
    check("sync: the button is enabled again", nodes()["sync"]["actionable"], str(nodes()["sync"]))

    # R7 — navigation: a row opens its detail; Back returns; old ids go away.
    click("task:0")
    on_detail = wait_for(lambda: text_of("detail_title") is not None, seconds=2.0)
    check("tap a row: the detail screen shows", on_detail, str(list(nodes())))
    check("detail: shows that task", text_of("detail_title") == "Buy milk", repr(text_of("detail_title")))
    gone = click("add").get("outcome")
    check("detail: the list's controls are not reachable", gone != "allowed", gone)
    click("back")
    back = wait_for(lambda: "add" in nodes(), seconds=2.0)
    check("back: the list is shown again", back, str(list(nodes())))
    check("back: the rows are intact", rows() == ["Buy milk", "Call Bob"], str(rows()))

    # The supervised record: what this run did, in order.
    act = rpc("activity")
    entries = act.get("actions", []) if isinstance(act, dict) else []
    verbs = [e.get("verb") for e in entries]
    check("activity records the run's actions in order",
          len(entries) >= 10 and verbs[:2] == ["set_text", "click"], str(verbs[:6]))

    failed = [n for n, ok in RESULTS if not ok]
    print(f"\n{len(RESULTS) - len(failed)}/{len(RESULTS)} passed")
    return len(failed)


if __name__ == "__main__":
    sys.exit(main())
