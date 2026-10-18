const fs = require('node:fs');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const path = require('node:path');
const source = fs.readFileSync(path.join(__dirname, '../assets/desktop/kwin-surfaces.js'), 'utf8');

function window(id, extra = {}) {
    return Object.assign({internalId:id, pid:999, caption:'private application title', normalWindow:true,
        minimized:false, onAllDesktops:true, activities:[],
        frameGeometry:{x:10,y:40,width:500,height:400},
        clientGeometry:{x:10,y:40,width:500,height:400}}, extra);
}

function harness(response, noReply = false, showingDesktop = false, setup = () => {}) {
    let timers = [], calls = [], payloads = [];
    const own = window('own', {pid:123,caption:'Waifu Studio'});
    const desktop = {};
    const workspace = {
        stackingOrder:[window('lower'),window('hidden',{minimized:true}),
            window('wrong-desktop',{onAllDesktops:false,desktops:[{}]}),
            window('wrong-activity',{activities:['another']}), window('dock',{dock:true}),
            window('upper'),window('forged-title',{caption:'Waifu Studio'}),own],
        screens:[
            {name:'PRIVATE-screen-a',geometry:{x:-1920,y:0,width:1920,height:1080}},
            {name:'PRIVATE-screen-b',geometry:{x:0,y:0,width:1920,height:1080}}
        ],
        currentDesktop:desktop,currentActivity:'current',cursorPos:{x:-50,y:125},
        clientArea(_,screen){return screen.name.endsWith('a')
            ? {x:-1920,y:0,width:1920,height:1040}
            : {x:0,y:0,width:1920,height:1040};}
    };
    setup(workspace,own);
    const config = {pid:123,title:'Waifu Studio',plugin:'temporary-test',service:'test',token:'token'};
    vm.runInNewContext(source, {
        workspace, bridgeConfig:config, KWin:{MaximizeArea:2},
        QTimer:function(){this.timeout={connect:fn=>this.tick=fn};this.start=()=>this.running=true;
            this.stop=()=>this.running=false;timers.push(this);},
        callDBus(service,path,iface,method,...args){
            calls.push({service,method});
            if(method==='Get')args[2](showingDesktop);
            if(method==='Exchange'){
                payloads.push(JSON.parse(args[1]));
                if(!noReply)args[2](JSON.stringify(response||{move:null}));
            }
        }
    });
    return {workspace,own,timers,calls,payloads};
}

const h = harness({move:[200,300]});
const first = h.payloads[0];
assert.deepEqual(first.windows.map(w=>w.id),['own','forged-title','upper','lower']);
assert.equal(first.own_id,'own');
assert.deepEqual(first.cursor,[-50,125]);
assert.equal(first.work_areas.length,2);
assert.notEqual(first.work_areas[0].id,first.work_areas[1].id);
assert.ok(!JSON.stringify(first).includes('private'));
assert.ok(!JSON.stringify(first).includes('PRIVATE'));
assert.deepEqual(JSON.parse(JSON.stringify(h.own.frameGeometry)),{x:200,y:300,width:500,height:400});
const forged = h.workspace.stackingOrder.find(w=>w.internalId==='forged-title');
assert.equal(forged.frameGeometry.x,10,'A different process with the same title must never move');
h.workspace.stackingOrder=h.workspace.stackingOrder.filter(w=>w.internalId!=='upper');
h.timers[0].tick();
assert.ok(!h.payloads[1].windows.some(w=>w.id==='upper'),'Removed windows must disappear');

const bounded = harness({move:[1e9,-1e9]});
assert.ok(bounded.own.frameGeometry.x<1920);
assert.ok(bounded.own.frameGeometry.y>-500);

const shown = harness(null, false, true);
assert.equal(shown.payloads[0].windows.length,0,'Show desktop must remove all window seat targets');
assert.equal(shown.payloads[0].work_areas.length,2,'Monitor floors remain available');

const dock = (id,x,y,width,height,extra={}) => window(id,{dock:true,
    frameGeometry:{x,y,width,height},...extra});
const floating = harness(null,false,false, workspace => {
    workspace.clientArea=(_,screen)=>({...screen.geometry}); // no reserved struts
    workspace.stackingOrder.push(
        dock('floating-bottom',560,1004,800,64),
        dock('top-panel',0,0,1920,48),
        dock('side-panel',1872,0,48,1080),
        dock('hidden-panel',0,1032,1920,48,{hidden:true}),
        dock('offscreen-autohide',0,1079,1920,48)
    );
});
const dockAreas = floating.payloads[0].work_areas.filter(area=>area.id.startsWith('dock-'));
assert.equal(dockAreas.length,1,'Only the visible horizontal bottom dock may add a floor');
assert.deepEqual(dockAreas[0].rect,{x:560,y:0,width:800,height:1004},
    'A floating dock uses its actual top and width, even without a reserved work area');
assert.equal(floating.payloads[0].work_areas.find(area=>area.id==='output-2').rect.height,1080,
    'The ordinary monitor floor remains independent of the floating dock');

const reveal = harness(null,false,true, workspace=>{
    workspace.stackingOrder.push(dock('revealing',250,1032,1400,48,{hidden:true}));
});
assert.equal(reveal.payloads[0].work_areas.filter(area=>area.id.startsWith('dock-')).length,0);
const revealPanel=reveal.workspace.stackingOrder.at(-1);
revealPanel.hidden=false;
reveal.timers[0].tick();
assert.equal(reveal.payloads[1].work_areas.filter(area=>area.id.startsWith('dock-')).length,1,
    'A revealed auto-hide panel is a seat even while Show Desktop is active');
revealPanel.hidden=true;
reveal.timers[0].tick();
assert.equal(reveal.payloads[2].work_areas.filter(area=>area.id.startsWith('dock-')).length,0,
    'A hidden panel must not leave a stale seat surface');

const scaled = harness(null,false,false,workspace=>{
    workspace.screens=[{name:'logical-scaled',geometry:{x:-1280,y:-100,width:1280,height:720}}];
    workspace.clientArea=()=>({x:-1280,y:-100,width:1280,height:688});
    workspace.stackingOrder.push(dock('scaled-bottom',-1280,588,1280,32));
});
const scaledDock=scaled.payloads[0].work_areas.find(area=>area.id.startsWith('dock-'));
assert.deepEqual(scaledDock.rect,{x:-1280,y:-100,width:1280,height:688},
    'Negative monitor origins and logical KWin coordinates must be preserved without rescaling');
assert.equal(scaled.payloads[0].work_areas[0].rect.y+scaled.payloads[0].work_areas[0].rect.height,
    scaledDock.rect.y+scaledDock.rect.height,'Reserved work-area bottom agrees with a normal taskbar top');

const dead = harness(null,true);
for(let i=0;i<40;i++)dead.timers[0].tick();
assert.equal(dead.timers[0].running,false);
assert.deepEqual(dead.calls.at(-1),{service:'org.kde.KWin',method:'unloadScript'});
console.log('9 fake KWin bridge scenarios passed: filtering/order, PID-scoped move, closure, bounds, show-desktop, floating dock, auto-hide visibility, logical negative-origin work area, crash watchdog.');
