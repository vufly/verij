#!/usr/bin/env python3
"""Visible review fixture with Enter/click activation, not production monitoring."""
import curses
from concurrent.futures import ThreadPoolExecutor
import json
from pathlib import Path
import sys

import controller


def review(screen, root, host):
    curses.curs_set(0)
    curses.mousemask(curses.ALL_MOUSE_EVENTS)
    screen.timeout(100)
    selected, status = 0, "Stock public API / no agent data / no Done acknowledgement"
    generation, queued, future, ticket = 0, None, None, None
    executor = ThreadPoolExecutor(max_workers=1)
    rows = [(0, "terminal 0 / tiled"), (1, "terminal 1 / tiled"), (2, "terminal 2 / floating"),
            (3, "terminal 3 / stack first"), (4, "terminal 4 / stack second")]
    while True:
        if future is not None and future.done():
            try:
                result = future.result()
                if ticket == generation:
                    status = result.get("status", "Observation refreshed") + " / ack=false"
            except Exception as error:
                if ticket == generation:
                    status = "UNAVAILABLE: " + str(error)[:100]
            future = None
        if future is None and queued is not None:
            operation, pane, ticket = queued
            queued = None
            if operation == "focus":
                active = ticket
                future = executor.submit(controller.activate, root, host, pane, lambda tag=active: generation == tag)
            elif operation == "query":
                future = executor.submit(controller.request, root, host, pane, "query")
            else:
                from demo import rebind
                future = executor.submit(rebind, root, host)
        screen.erase()
        height, width = screen.getmaxyx()
        def put(y, text, style=0):
            if y < height and width > 1:
                try:
                    screen.addnstr(y, 0, text, width - 1, style)
                except curses.error:
                    pass
        put(0, f"STOCK REVIEW / HOST {host.upper()}", curses.A_BOLD)
        put(1, "Disposable control fixture")
        put(2, "No production agent rows or Done")
        for index, (_, label) in enumerate(rows):
            put(4 + index, ("> " if selected == index else "  ") + label, curses.A_REVERSE if selected == index else 0)
        put(9, "j/k, arrows: select only")
        put(10, "Enter/click: activate Workspace")
        put(11, "Click this pane to return")
        put(12, "q: query / r: rebind")
        put(14, status)
        screen.refresh()
        key = screen.getch()
        if key != -1:
            with (root / f"sidebar-{host}-events.ndjson").open("a") as output:
                output.write(json.dumps({"key": key, "selected": selected}) + "\n")
        activate = False
        if key in (ord("j"), curses.KEY_DOWN):
            selected = min(selected + 1, len(rows) - 1)
            status = "Selection only; no navigation or acknowledgement"
        elif key in (ord("k"), curses.KEY_UP):
            selected = max(selected - 1, 0)
            status = "Selection only; no navigation or acknowledgement"
        elif key in (10, 13, curses.KEY_ENTER):
            activate = True
        elif key == curses.KEY_MOUSE:
            try:
                _, x, y, _, buttons = curses.getmouse()
            except curses.error:
                with (root / f"sidebar-{host}-events.ndjson").open("a") as output:
                    output.write(json.dumps({"incomplete_mouse_event_ignored": True}) + "\n")
                continue
            with (root / f"sidebar-{host}-events.ndjson").open("a") as output:
                output.write(json.dumps({"mouse_x": x, "mouse_y": y, "buttons": buttons}) + "\n")
            if 4 <= y < 4 + len(rows) and buttons & (curses.BUTTON1_CLICKED | curses.BUTTON1_PRESSED | curses.BUTTON1_DOUBLE_CLICKED):
                selected, activate = y - 4, True
        try:
            if activate:
                generation += 1
                queued = ("focus", rows[selected][0], generation)
                status = "Activation queued / ack=false"
            elif key == ord("q"):
                queued = ("query", rows[selected][0], generation)
                status = "Passive query queued / ack=false"
            elif key == ord("r"):
                generation += 1
                queued = ("rebind", rows[selected][0], generation)
                status = "Keyboard registration queued / ack=false"
        except Exception as error:
            status = "UNAVAILABLE: " + str(error)[:100]


if __name__ == "__main__":
    root, host = Path(sys.argv[1]).resolve(strict=True), sys.argv[2]
    curses.wrapper(review, root, host)
