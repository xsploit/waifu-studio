"""Build a local frame-by-frame review desk from native quad-view captures.

No clip is automatically accepted. PNG stepping uses the exact saved frame.
Optional MP4s are viewing copies; lossless source PNGs stay in the review folder.
"""
import argparse
import json
from pathlib import Path
import subprocess


PAGE = r'''<!doctype html><html lang="en"><meta charset="utf-8"><title>Hikari motion review</title>
<style>*{box-sizing:border-box}body{margin:0;background:#18191b;color:#eceef0;font:16px system-ui}main{max-width:1400px;margin:24px auto;padding:0 24px}h1{font-size:26px}p{color:#b7bbc1;line-height:1.5}button,select,input,textarea{font:inherit;background:#2d3035;color:inherit;border:1px solid #4c515b;border-radius:7px;padding:9px}button{cursor:pointer}button:hover{background:#3f454e}.bar{display:flex;gap:10px;align-items:center;flex-wrap:wrap;margin:12px 0}.stage{height:min(69vh,960px);aspect-ratio:4/3;max-width:100%;margin:auto;position:relative;overflow:hidden;background:#262a30;border-radius:8px}.stage img{width:100%;height:100%;object-fit:contain;transform-origin:top left}.stage[data-view="front"] img{width:200%;height:200%}.stage[data-view="side"] img{width:200%;height:200%;transform:translateX(-50%)}.stage[data-view="rear"] img{width:200%;height:200%;transform:translateY(-50%)}.stage[data-view="other-side"] img{width:200%;height:200%;transform:translate(-50%,-50%)}#seek{flex:1;min-width:200px}textarea{width:100%;height:70px}.good{border-color:#63ae78}.bad{border-color:#d87979}.badge{padding:6px 10px;border-radius:8px;background:#30343a}a{color:#9dbdff}#clip{max-width:600px}#frame{font-variant-numeric:tabular-nums}</style>
<main><h1>Hikari · complete motion review</h1><p id="summary"></p>
<p>Top left: front · Top right: side · Bottom left: rear · Bottom right: other side. Exact source-frame stepping, actual native retargeter and spring physics. Captured does not mean approved. These are individual clips; loop seams and transitions between different clips need separate review.</p>
<div class="bar"><select id="clip"></select><button id="previousClip">Previous clip</button><button id="nextClip">Next clip</button><span class="badge" id="state"></span></div>
<div class="stage" id="stage" data-view="all"><img id="image" alt="Native Hikari motion frame"></div>
<div class="bar"><button id="start">First</button><button id="back">−1 frame</button><button id="play">Play</button><button id="forward">+1 frame</button><button id="end">Last</button><select id="speed"><option value="1">1×</option><option value="0.5">½×</option><option value="0.25">¼×</option></select><select id="view"><option value="all">Four views</option><option>front</option><option>side</option><option>rear</option><option>other-side</option></select><span id="frame"></span></div>
<div class="bar"><input id="seek" type="range" min="0" value="0"><a id="raw" target="_blank">Open exact PNG</a><a id="video" target="_blank">MP4 copy</a></div>
<textarea id="notes" placeholder="Record failing frame numbers, arms, hands, jacket, hair, start/return pose…"></textarea>
<div class="bar"><button id="reject" class="bad">Reject this clip</button><button id="pending">Needs review</button><button id="accept" class="good">Accept individual clip</button><button id="export">Export review JSON</button></div>
<p>Acceptance is per Hikari model and this renderer. Decisions are saved locally in this browser and can be exported; they do not silently enable animations in the app.</p></main>
<script>const data=__DATA__; const $=id=>document.getElementById(id);let index=0,frame=0,playing=false,last=0;const key='waifu-hikari-motion-review-v1';let reviews={};try{reviews=JSON.parse(localStorage.getItem(key)||'{}')}catch{};
const ready=data.clips.filter(c=>c.captured_frames>0);$('summary').textContent=`${ready.length}/${data.total} clips captured · ${data.captured_frames.toLocaleString()} frames · 30 FPS · eyes excluded from body retargeting · no collision acceptance inferred`;
for(const c of ready){const option=document.createElement('option');option.value=ready.indexOf(c);option.textContent=c.clip;$('clip').append(option)}
function save(state){const c=ready[index];reviews[c.clip]={status:state,notes:$('notes').value,sha256:c.sha256,model:data.model};localStorage.setItem(key,JSON.stringify(reviews));$('state').textContent=state}
function show(){if(!ready.length)return;const c=ready[index];frame=Math.max(0,Math.min(frame,c.captured_frames-1));const path=`frames/${c.stem}/frame_${String(frame).padStart(4,'0')}.png`;$('image').src=path;$('raw').href=path;$('seek').max=c.captured_frames-1;$('seek').value=frame;$('frame').textContent=`${frame+1}/${c.captured_frames} · ${(frame/30).toFixed(3)}s`;$('video').href=`videos/${c.stem}.mp4`;$('video').style.display=c.video?'':'none'}
function select(i){if(!ready.length)return;playing=false;$('play').textContent='Play';index=(i+ready.length)%ready.length;frame=0;$('clip').value=index;const c=ready[index];const r=reviews[c.clip]||{status:c.visual_status,notes:c.notes||''};$('notes').value=r.notes||'';$('state').textContent=r.status;show()}
function step(n){playing=false;$('play').textContent='Play';frame+=n;show()}
$('clip').onchange=()=>select(Number($('clip').value));$('previousClip').onclick=()=>select(index-1);$('nextClip').onclick=()=>select(index+1);$('back').onclick=()=>step(-1);$('forward').onclick=()=>step(1);$('start').onclick=()=>{frame=0;step(0)};$('end').onclick=()=>{frame=ready[index].captured_frames-1;step(0)};$('seek').oninput=()=>{frame=+$('seek').value;step(0)};$('view').onchange=()=>{$('stage').dataset.view=$('view').value};$('play').onclick=()=>{playing=!playing;last=0;$('play').textContent=playing?'Pause':'Play'};
$('reject').onclick=()=>save('rejected');$('pending').onclick=()=>save('needs_review');$('accept').onclick=()=>save('accepted_individual_clip');$('notes').onchange=()=>save(reviews[ready[index].clip]?.status||'needs_review');$('export').onclick=()=>{const a=document.createElement('a');a.href=URL.createObjectURL(new Blob([JSON.stringify({model:data.model,scope:'individual clips, transitions not accepted',reviews},null,2)],{type:'application/json'}));a.download='hikari-motion-review.json';a.click();URL.revokeObjectURL(a.href)};
document.onkeydown=e=>{if(['INPUT','SELECT','TEXTAREA'].includes(document.activeElement.tagName))return;if(e.key==='ArrowRight'){e.preventDefault();step(1)}if(e.key==='ArrowLeft'){e.preventDefault();step(-1)}};
function tick(t){if(playing&&ready.length){const interval=1000/(30*Number($('speed').value));if(!last)last=t;if(t-last>=interval){last=t;frame++;if(frame>=ready[index].captured_frames){frame=ready[index].captured_frames-1;playing=false;$('play').textContent='Play'}show()}}requestAnimationFrame(tick)}select(0);requestAnimationFrame(tick);</script></html>'''


def build(root, audit, encode):
    source = json.loads(audit.read_text(encoding="utf-8"))
    findings_path = root / "review-findings.json"
    findings = json.loads(findings_path.read_text(encoding="utf-8")) if findings_path.exists() else {}
    result = {"model": "81453f3567d8-hikari-wardrobe.vrm", "total": source["total"], "captured_frames": 0, "clips": []}
    for clip in source["clips"]:
        item = dict(clip)
        if clip["clip"] in findings:
            finding = findings[clip["clip"]]
            if finding.get("sha256") != clip["sha256"]:
                raise ValueError(f"Review finding refers to different source: {clip['clip']}")
            item.update(finding)
        stem = Path(clip["clip"]).stem
        item.update(stem=stem, captured_frames=0, video=False)
        directory = root / "frames" / stem
        receipt = directory / "receipt.json"
        if receipt.exists():
            native = json.loads(receipt.read_text(encoding="utf-8"))
            frames = sorted(directory.glob("frame_*.png"))
            expected = native["captured_frames"]
            if len(frames) != expected or any(p.name != f"frame_{n:04}.png" for n,p in enumerate(frames)):
                raise ValueError(f"Missing capture frames for {stem}")
            if expected != clip["frames"]:
                raise ValueError(f"Source/capture count mismatch for {stem}: {expected}/{clip['frames']}")
            item.update(captured_frames=expected, native_receipt=native)
            result["captured_frames"] += expected
            if encode:
                output = root / "videos" / f"{stem}.mp4"
                output.parent.mkdir(exist_ok=True)
                if not output.exists():
                    subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-framerate", "30", "-i", str(directory / "frame_%04d.png"), "-vf", "format=yuv420p", "-c:v", "libx264", "-preset", "fast", "-crf", "18", "-g", "30", "-bf", "0", "-movflags", "+faststart", str(output)], check=True)
            item["video"] = (root / "videos" / f"{stem}.mp4").exists()
        result["clips"].append(item)
    (root / "review-manifest.json").write_text(json.dumps(result, indent=2), encoding="utf-8")
    (root / "index.html").write_text(PAGE.replace("__DATA__", json.dumps(result).replace("</", "<\\/")), encoding="utf-8")
    print(json.dumps({"captured_clips": sum(bool(c["captured_frames"]) for c in result["clips"]), "captured_frames": result["captured_frames"], "total": result["total"]}))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    parser.add_argument("audit", type=Path)
    parser.add_argument("--encode", action="store_true")
    args = parser.parse_args()
    build(args.root, args.audit, args.encode)
