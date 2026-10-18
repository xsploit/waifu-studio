// Runtime-only KWin 6 bridge. Rust prepends a JSON `bridgeConfig` declaration.
// No titles, resource classes, screen names, or contents leave this script.
// APIs: https://develop.kde.org/docs/plasma/kwin/api/
// KWin exports QTimer: src/scripting/scripting.cpp (Plasma/6.5 and later).
(function () {
    "use strict";
    var timer = new QTimer();
    timer.interval = 50;
    var waiting = false;
    var pendingTicks = 0;
    var stopped = false;
    var outputIds = {};
    var nextOutputId = 1;

    function rect(r) {
        return { x: r.x, y: r.y, width: r.width, height: r.height };
    }

    function isOwn(w) {
        // A title alone is insufficient: never move a different process.
        return w.pid === bridgeConfig.pid && w.caption === bridgeConfig.title;
    }

    function onCurrentDesktop(w) {
        if (w.onAllDesktops) return true;
        var desktops = w.desktops;
        var current = typeof workspace.currentDesktopForScreen === "function"
            ? workspace.currentDesktopForScreen(w.output) : workspace.currentDesktop;
        for (var i = 0; i < desktops.length; i++) {
            if (desktops[i] === current) return true;
        }
        return false;
    }

    function onCurrentActivity(w) {
        var activities = w.activities;
        return activities.length === 0 || activities.indexOf(workspace.currentActivity) !== -1;
    }

    function finish() {
        if (stopped) return;
        stopped = true;
        timer.stop();
        // Unload only this randomly named, temporary script, including on a
        // dead service / app crash. Nothing is installed in KWin settings.
        callDBus("org.kde.KWin", "/Scripting", "org.kde.kwin.Scripting",
            "unloadScript", bridgeConfig.plugin);
    }

    function applyMove(command, own, areas) {
        if (!command || !own || own.move || own.resize || areas.length === 0) return;
        var x = command[0], y = command[1];
        if (typeof x !== "number" || typeof y !== "number" || !isFinite(x) || !isFinite(y)) return;
        var client = own.clientGeometry;
        // Keep at least 24 logical pixels reachable on the nearest screen.
        // Separate outputs are used, so a gap between monitors is never a seat.
        var best = null, distance = Infinity;
        for (var i = 0; i < areas.length; i++) {
            var r = areas[i].rect;
            var nx = Math.max(r.x - client.width + 24, Math.min(x, r.x + r.width - 24));
            var ny = Math.max(r.y - client.height + 24, Math.min(y, r.y + r.height - 24));
            var d = (nx - x) * (nx - x) + (ny - y) * (ny - y);
            if (d < distance) { distance = d; best = [nx, ny]; }
        }
        var f = own.frameGeometry;
        // Requested origin is the client origin; preserve the decoration offset.
        own.frameGeometry = {
            x: f.x + best[0] - client.x,
            y: f.y + best[1] - client.y,
            width: f.width,
            height: f.height
        };
    }

    function tick() {
        if (stopped) return;
        if (waiting) {
            if (++pendingTicks >= 40) finish();
            return;
        }
        waiting = true;
        pendingTicks = 0;
        // KWin's show-desktop state is public on D-Bus, but absent from the
        // scripting workspace wrapper. Read it asynchronously so hidden
        // windows cannot remain seat targets while the desktop is shown.
        callDBus("org.kde.KWin", "/KWin", "org.freedesktop.DBus.Properties", "Get",
            "org.kde.KWin", "showingDesktop", function (showingDesktop) {
                if (stopped) return;
                if (typeof showingDesktop !== "boolean") { finish(); return; }
                publish(showingDesktop);
            });
    }

    function publish(showingDesktop) {
        var windows = workspace.stackingOrder;
        var records = [];
        var docks = [];
        var own = null;
        // KWin stacking order is bottom to top; reverse to match smaller z on top.
        for (var i = windows.length - 1; i >= 0; i--) {
            var w = windows[i];
            if (isOwn(w)) {
                if (own !== null) { finish(); return; } // ambiguous owner, fail closed
                own = w;
            }
            if (w.minimized || w.hidden || w.deleted || w.opacity === 0
                || !onCurrentDesktop(w) || !onCurrentActivity(w)) continue;
            // Visible bottom panels may float or auto-hide without reserving
            // a work-area strut. Their actual top edge is still a seat line.
            if (w.dock) {
                docks.push({ id: String(w.internalId), rect: rect(w.frameGeometry) });
                continue;
            }
            if (showingDesktop || w.desktopWindow || !w.normalWindow) continue;
            // The internal UUID works for both Wayland and XWayland windows.
            records.push({ id: String(w.internalId), rect: rect(w.frameGeometry) });
        }
        var areas = [];
        var screens = workspace.screens;
        for (var j = 0; j < screens.length; j++) {
            // MaximizeArea is per-output and excludes panel struts. WorkArea
            // spans outputs and must not be treated as one monitor floor.
            var key = "$" + String(screens[j].name);
            if (!outputIds[key]) outputIds[key] = "output-" + nextOutputId++;
            var desktop = typeof workspace.currentDesktopForScreen === "function"
                ? workspace.currentDesktopForScreen(screens[j]) : workspace.currentDesktop;
            areas.push({ id: outputIds[key], rect: rect(workspace.clientArea(
                KWin.MaximizeArea, screens[j], desktop)) });
            var screen = screens[j].geometry;
            if (!screen) continue;
            for (var k = 0; k < docks.length; k++) {
                var panel = docks[k].rect;
                var gap = screen.y + screen.height - panel.y - panel.height;
                var left = Math.max(screen.x, panel.x);
                var right = Math.min(screen.x + screen.width, panel.x + panel.width);
                // Restrict to horizontal bottom docks. Top and vertical panels
                // must never project a spurious floor across the desktop.
                if (panel.width < panel.height || panel.height <= 0
                    || panel.height > screen.height * 0.25 || left >= right
                    || panel.y < screen.y + screen.height * 0.5
                    || gap < -1 || gap > Math.max(80, panel.height * 2)) continue;
                // WorkArea seats use their rectangle's bottom edge. Project
                // from output top to dock top, only over this dock's width.
                areas.push({ id: "dock-" + docks[k].id + "-" + outputIds[key], rect: {
                    x: left, y: screen.y, width: right - left,
                    height: panel.y - screen.y
                } });
            }
        }
        var cursor = workspace.cursorPos;
        var payload = JSON.stringify({
            windows: records,
            work_areas: areas,
            own_id: own ? String(own.internalId) : null,
            own_rect: own ? rect(own.clientGeometry) : null,
            own_interactive_move: own ? Boolean(own.move) : false,
            cursor: [cursor.x, cursor.y]
        });
        waiting = true;
        pendingTicks = 0;
        callDBus(bridgeConfig.service, "/org/waifustudio/DesktopBridge",
            "org.waifustudio.DesktopBridge", "Exchange", bridgeConfig.token, payload,
            function (reply) {
                waiting = false;
                if (stopped) return;
                try {
                    var response = JSON.parse(reply);
                    if (response.stop) { finish(); return; }
                    applyMove(response.move, own, areas);
                } catch (_) { finish(); }
            });
    }

    timer.timeout.connect(tick);
    timer.start();
    tick();
}());
