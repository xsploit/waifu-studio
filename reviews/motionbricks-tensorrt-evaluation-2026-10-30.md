# MotionBricks TensorRT evaluation

This is a local, experimental evaluation. It does not add MotionBricks to Waifu Studio's automatic animation playback or establish that clipping has been fixed.

## Source and setup

- NVIDIA MotionBricks 0.2.0, repository commit `54fc563aeaaf819a753d72cc9fdc6713989cdd43`, installed separately under `work/motionbricks-nvidia`.
- SOMA mixed-FP16 ONNX file: 380,327,058 bytes, verified against its Git LFS SHA-256. The Windows TensorRT plan is 386,436,332 bytes and is specific to its build environment.
- Windows test GPU: GeForce RTX 5060 Ti 16 GB, driver 610.88; Warp 1.17.0, CUDA 12.9, TensorRT 10.8.0.43.
- The first build and two generations took approximately 145.6 seconds and the process temporarily used about 7 GB of resident system memory. This includes setup and engine compilation; it does not measure steady-state app cost.
- With the cached engine and kernels, a subsequent process reported about 0.78 seconds for startup and both generations. Two synchronized generation calls took approximately 30 and 2.4 milliseconds. This is a two-call smoke test, not a representative latency, VRAM, or frame-time benchmark.

NVIDIA's code, model weights, and sample motions have separate terms. The test uses an NVIDIA sample walking clip for context and target poses; keep it for local evaluation and review the applicable sample terms before redistribution. The official repository and model cards are at https://github.com/NVlabs/motionbricks.

## What was tested

NVIDIA's headless example generated 26 and 50 finite SOMA pose frames in chained calls. The saved local test artifact and generator are in `work/motionbricks_probe`; `Generate-Review.cmd` reruns the Windows TensorRT example and writes an experimental VRMA.

The experiment converts the example's Z-up output back to SOMA Y-up, preserves the supplied 77-node rest hierarchy, and maps 22 bones explicitly. This mapping matters because SOMA `LeftLeg`/`LeftShin` mean upper/lower leg, while Waifu Studio's generic Mixamo aliases interpret `LeftLeg` as the lower leg. The preview maps neither eye nor jaw and contains no eye or jaw animation tracks. These are adapter choices for this experiment, not proof of a general SOMA-to-VRM retargeter.

Waifu Studio's offscreen production renderer successfully imported the experimental VRMA and mapped 22 bones on Hikari. All 76 frames were rendered at 30 FPS from both front and side views, with horizontal camera following and the existing arm-clearance guard disabled. The frames and review sheets are under `work/motionbricks_probe`. Visual inspection shows fingers/hands entering or disappearing behind the skirt in several frames; this motion is not accepted as collision-free. Hair and body simulation behavior in the offscreen renderer is not a substitute for live desktop testing.

The result demonstrates GPU generation plus an offline import and preview. It is not an in-app real-time integration: there is no app-facing continuous pose stream, decision-model control, lifecycle/resource management, arbitrary VRM support, or validated collision/cloth solution.

## Linux check

The isolated `WaifuStudioTest` WSL environment detected the RTX 5060 Ti, installed the same Python dependencies separately under `/root/motionbricks-linux-env`, and initialized Warp on CUDA. Its first TensorRT engine build was terminated when Windows reported about 1.6 GiB of free physical memory. No Linux engine or generated output was verified. A native Linux GPU run and its performance remain untested.
