// Execute the shipped KWin script against owned fake compositor objects.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const test = require('node:test');
const source = fs.readFileSync(path.join(__dirname, '../assets/desktop/kwin-surfaces.js'), 'utf8');

function fixture({ reserved = true, panel = true, original = false, impostor = false } = {}) {
    const output = { name: 'fixture', geometry: { x: 0, y: 0, width: 1920, height: 1080 } };
    const base = { onAllDesktops: true, activities: [], output, opacity: 1 };
    const own = { ...base, pid: impostor ? 99 : 42, caption: 'Fixture', normalWindow: true,
        internalId: 'own', keepAbove: original,
        frameGeometry: { x: 100, y: 400, width: 420, height: 640 },
        clientGeometry: { x: 100, y: 400, width: 420, height: 640 } };
    const dock = { ...base, dock: true, internalId: 'panel',
        frameGeometry: { x: 0, y: 1040, width: 1920, height: 40 } };
    let tick, reply = { keep_above: true }, payload, unloaded = false;
    const context = {
        bridgeConfig: { pid: 42, title: 'Fixture', service: 'fixture', token: 'owned', plugin: 'owned' },
        QTimer: function () { this.timeout = { connect: fn => tick = fn }; this.start = () => {}; this.stop = () => {}; },
        KWin: { MaximizeArea: 0 },
        workspace: {
            screens: [output], cursorPos: { x: 100, y: 500 },
            get stackingOrder() { return panel ? (own.keepAbove ? [dock, own] : [own, dock]) : [own]; },
            clientArea: () => ({ x: 0, y: 0, width: 1920, height: reserved ? 1040 : 1080 })
        },
        callDBus: (...args) => {
            const method = args[3], callback = args.at(-1);
            if (method === 'Get') callback(false);
            else if (method === 'Exchange') { payload = JSON.parse(args[5]); callback(JSON.stringify(reply)); }
            else if (method === 'unloadScript') unloaded = true;
            else throw Error(`Unexpected method ${method}`);
        }
    };
    vm.runInNewContext(source, context);
    return { own, tick: value => { reply = value; tick(); }, payload: () => payload, unloaded: () => unloaded };
}

test('above-panel request targets only the owning window and reports the next actual order', () => {
    const f = fixture();
    assert.equal(f.payload().own_above_panels, false);
    assert.equal(f.own.keepAbove, true);
    f.tick({ keep_above: true });
    assert.equal(f.payload().own_above_panels, true);
    f.tick({ keep_above: false });
    assert.equal(f.own.keepAbove, false);
});

test('stop restores original stacking rather than leaving studio mode above everything', () => {
    for (const original of [false, true]) {
        const f = fixture({ original });
        f.tick({ keep_above: !original });
        assert.equal(f.own.keepAbove, !original);
        f.tick({ stop: true });
        assert.equal(f.own.keepAbove, original);
        assert.equal(f.unloaded(), true);
    }
});

test('monitor bottom without a panel is a placement bound and never a taskbar seat', () => {
    const f = fixture({ reserved: false, panel: false });
    assert.equal(f.payload().work_areas.length, 1);
    assert.equal(f.payload().work_areas[0].seat, false);
});

test('floating panel without reserved space uses its actual top for seating', () => {
    const f = fixture({ reserved: false });
    const floors = f.payload().work_areas.filter(area => area.seat);
    assert.equal(floors.length, 1);
    assert.equal(floors[0].rect.y + floors[0].rect.height, 1040);
});

test('same title from another PID never acquires the stacking flag', () => {
    const f = fixture({ impostor: true });
    assert.equal(f.own.keepAbove, false);
    assert.equal(f.payload().own_id, null);
});
