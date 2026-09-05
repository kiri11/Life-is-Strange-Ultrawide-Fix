# Life is Strange: Double Exposure - Ultrawide Reverse Engineering & Research

**Game:** *Life is Strange: Double Exposure*
**Engine:** Unreal Engine 5.2.1 (Shipping x86-64)
**Binary:** `Chronos-Win64-Shipping.exe` (~134.5 MB)
**Reference Resolution Tested:** 5120x2160 (21.33:9)

This describes the fix as it ships, and only that. Approaches that were tried and dropped are not documented here - with one deliberate exception: **section 6** keeps them as warnings, because each one looked right at the time and cost real work to disprove. Superseded iterations of the patch itself live in git history.

---

## 1. The Current Solution

Three code changes (one 2-byte branch edit + two code caves) and **no aspect-ratio constants**, applied in memory at every launch by the loader library (`crates/loader`, installed as `winhttp.dll` next to the executable - see 8). All code sites are located by unique byte signatures with the documented file offsets as fast paths (in this build RVA = file offset + 0xA00), so the patch survives game updates that shift offsets; if a signature disappears, nothing is written and the loader's log says so. The executable on disk is never modified.

```
Target File: Chronos/Binaries/Win64/Chronos-Win64-Shipping.exe
------------------------------------------------------------------------------
0x440ABC6: 02 -> FF                         disable MajorAxisFOV branch
0x440ABCF: 01 -> FF                         disable MaintainXFOV branch
                                            (forces the Hor+ MaintainYFOV path)
0x441A14C: movzx -> call caveA + 2-byte nop aspect-gated unconstrain + divisor pin
0x4005B87: call displacement -> caveB       cine (loading) views kept boxed
caveA (40 bytes), caveB (18 bytes):         written into int3 padding; located
                                            and linked at patch time
------------------------------------------------------------------------------
```

Result matrix (field-verified at 5120x2160):

| Surface | Behavior |
|---|---|
| Cutscenes & dialogues | Full-width Hor+, 0% vertical crop |
| Free-roam exploration | Full-width Hor+, 0% vertical crop |
| Dialogue / cutscene hand-off | Seamless - no pillarbox sweep, no zoom, no snap |
| Photo mode / Polaroids | Correct proportions; pipeline bit-identical to vanilla |
| Main menu | Full-width Hor+ |
| Loading transitions | Overlay covers the full screen (with the UI patch, section 9) |
| 16:9 displays | Behavior-neutral |

**No aspect-ratio constant is written.** In particular the player-camera constructor default at `0x23E665C` and the photo projection table at `0x69C8A8C` are left exactly as shipped. Neither governs what its location suggests: free-roam already renders Hor+ through cave A, and the photo pipeline is correct *because* both constants keep their 16:9 values (section 4; measured in section 10). Writing a monitor aspect into them is an inviting mistake - section 6.2 is there to head it off.

`Engine.ini` is not required for the camera fix. The installer can optionally write chromatic-aberration and anti-blur TSR settings; see the README.

---

## 2. How It Works

### 2a. UE5 Already Contains Perfect Hor+ Math
`FMinimalViewInfo::CalculateProjectionMatrixGivenViewRectangle` (function fragment at file `0x440AAB0`, VA `0x14440B4B0`) implements, for unconstrained cameras in the `MaintainYFOV` branch (confirmed against Epic's `CameraTypes.cpp`):

```cpp
const bool bMaintainXFOV =
    ((SizeX > SizeY) && (AspectRatioAxisConstraint == AspectRatio_MajorAxisFOV)) ||
    (AspectRatioAxisConstraint == AspectRatio_MaintainXFOV) ||
    (ViewInfo.ProjectionMode == ECameraProjectionMode::Orthographic);
...
if (!bMaintainXFOV && ViewInfo.AspectRatio != 0.f && ...)
{
    // The view-info FOV is horizontal. Convert it to a vertical FOV using the
    // aspect ratio it was AUTHORED with, then expand horizontally.
    const float HalfXFOV = FMath::DegreesToRadians(FMath::Max(0.001f, ViewInfo.FOV) / 2.f);
    const float HalfYFOV = FMath::Atan(FMath::Tan(HalfXFOV) / ViewInfo.AspectRatio);
    MatrixHalfFOV = HalfYFOV;
}
```

With `bConstrainAspectRatio == false` and this branch active, a camera authored horizontal-for-16:9 renders **true Hor+ with unchanged vertical framing** on any wider viewport - the engine does the `atan(tan(hFOV/2)/authoredAspect)` conversion itself. No runtime FOV hook is needed; the fix reduces to (a) forcing this branch and (b) deciding which cameras are unconstrained.

**Critical rule:** `ViewInfo.AspectRatio` is the divisor - it must remain the *authored* value for any camera that is unconstrained. Overwriting it with the monitor ratio shrinks the computed vertical FOV by exactly the ultrawide factor (Vert- crop, perceived as zoom).

### 2b. Forcing the Hor+ Branch - `0x440ABC6` / `0x440ABCF`
The resolved axis-constraint enum arrives in `dl`:

```
0x440ABC0: 3B C1                  cmp  eax, ecx        ; SizeX vs SizeY
0x440ABC2: 7E 09                  jle  +9
0x440ABC4: 80 FA 02               cmp  dl, 2           ; MajorAxisFOV?
0x440ABC7: 0F 84 D2 01 00 00      je   Vert- path
0x440ABCD: 80 FA 01               cmp  dl, 1           ; MaintainXFOV?
0x440ABD0: 0F 84 C9 01 00 00      je   Vert- path
0x440ABD6: 0F B6 53 50            movzx edx, byte [rbx+0x50]   ; ProjectionMode
0x440ABDA: 80 FA 01               cmp  dl, 1           ; Orthographic?
```

Rewriting the two immediates to `FF` makes both comparisons unsatisfiable for any real enum value (0-2), so **every perspective camera takes the Hor+ path**, regardless of `ULocalPlayer` settings, sequencer overrides, or the per-camera `TOptional<EAspectRatioAxisConstraint>` (UE 5.1+) - all resolved into `dl` before this branch. The orthographic check is preserved.

This also fixes the **post-cutscene zoom bug** at its root: `ULevelSequencePlayer` overrides the local player's `AspectRatioAxisConstraint` to `MaintainXFOV` during playback and can fail to restore it afterwards (early-return when the final camera cut lands on an identical view target; UE-76517, UE forum threads 445043 / 491662). On a wide viewport a leaked `MaintainXFOV` renders gameplay vertically cropped ("zoomed in"). With the branch forced, the leaked value is irrelevant.

### 2c. Cave A: Aspect-Gated Unconstrain - `0x441A14C`
`UCameraComponent::GetCameraView` (true entry VA `0x144419EC0`) copies component state (`rbx` = component: FieldOfView `+0x2A0`, AspectRatio `+0x2B0`, bitfield `+0x2B4`, ProjectionMode `+0x2B5`) into the output `FMinimalViewInfo` (`rdi`: FOV `+0x30`, AspectRatio `+0x48`, bitfield `+0x4C`, ProjectionMode `+0x50`). The constraint flag is merged bit-exactly:

```
0x441A14C: 0F B6 83 B4 02 00 00   movzx eax, byte [rbx+0x2B4]   ; <- patched
0x441A153: 33 47 4C               xor   eax, [rdi+0x4C]
0x441A156: 83 E0 01               and   eax, 1                  ; bit 0 = bConstrainAspectRatio
0x441A159: 31 47 4C               xor   [rdi+0x4C], eax
```

Critically, the component's `AspectRatio` is copied into the output view in the two instructions *immediately before* the patched site, so inside the cave `rdi` already points at a view whose aspect can be rewritten:

```
0x441A143: 8B 83 B0 02 00 00      mov   eax, [rbx+0x2B0]        ; component AspectRatio
0x441A149: 89 47 48               mov   [rdi+0x48], eax         ; view.AspectRatio = it
```

The 7-byte `movzx` becomes `call caveA` + 2-byte nop:

```
caveA: 0F B6 83 B4 02 00 00   movzx eax, byte [rbx+0x2B4]   ; original instruction
       8B 8B B0 02 00 00      mov   ecx, [rbx+0x2B0]        ; component AspectRatio
       81 F9 00 00 E0 3F      cmp   ecx, 1.75f              ; IEEE754 order == integer order
       76 12                  jbe   done
       81 F9 <display*1.002>  cmp   ecx, upper bound
       73 0A                  jae   done
       83 E0 FE               and   eax, -2                 ; clear bConstrainAspectRatio
       C7 47 48 39 8E E3 3F   mov   dword [rdi+0x48], 1.7777778f   ; pin the divisor
done:  C3                     ret
```

Every camera authored **narrower than the display** is unconstrained and renders Hor+ - the cutscene, dialogue, menu and free-roam cameras alike. Square capture cameras (~1.0) fall below the window and keep their constraint and classic behaviour, which is what protects the photo pipeline. `ecx` is reloaded immediately after the patched site, so nothing is clobbered; the cave uses no stack and needs no alignment.

**The divisor pin is the part that matters most**, and it is not obvious from static reading. The game *animates* a camera's `AspectRatio` from the authored 16:9 up to the viewport aspect when it hands control back from a dialogue - its letterbox-open animation. Under the forced `MaintainYFOV` branch that member is the FOV divisor, so without the pin the animation is re-read as a vertical-FOV change: a zoom-in while the ramp climbs, a pillarbox sweep if the ramp leaves the gate window, and a framing snap when the gameplay camera takes over. Writing the authored aspect back into `[rdi+0x48]` restores the rule from 2a - the divisor must be the aspect the FOV was authored for. Full measurements in section 10.

The gate's upper bound is `display aspect * 1.002` rather than the display aspect exactly, so the ramp's endpoint stays inside the window despite float rounding and any easing overshoot.

### 2d. Cave B: Cine Views Kept Boxed - `0x4005B86`
In this game `UCineCameraComponent` drives **loading/transition views**, not cutscenes (Deck Nine's cutscene cameras are their own component classes - see 3c). Cine sensors are 16:9, so cave A would unconstrain them; cave B overrides that. The entire binary contains exactly **one direct call** to `UCameraComponent::GetCameraView`: the `Super::GetCameraView` call inside `UCineCameraComponent::GetCameraView`:

```
0x4005B6F: mov  rdi, r8          ; rdi = FMinimalViewInfo& DesiredView (non-volatile)
...
0x4005B7D: mov  r8, rdi
0x4005B80: movaps xmm1, xmm6
0x4005B83: mov  rcx, rbx
0x4005B86: call 0x144419EC0      ; Super::GetCameraView  <- rerouted to caveB
```

```
caveB: 48 83 EC 28          sub  rsp, 0x28        ; shadow space for Super
       E8 <rel32>           call 0x144419EC0      ; original Super call
       48 83 C4 28          add  rsp, 0x28
       80 4F 4C 01          or   byte [rdi+0x4C], 1   ; bConstrainAspectRatio = true
       C3                   ret
```

Every other invocation of the base `GetCameraView` is a virtual call (non-cine cameras), so this affects exactly the cine class. `rdi` survives the call (callee-saved); stack alignment and Win64 shadow-space rules are respected.

### 2e. Cave Placement
Both caves are written into `int3` inter-function padding runs in the code sections (`.text` here; the loader walks every section flagged executable, which Reunion's renamed sections will need) - the first run large enough, found at launch; rel32 displacements computed then too). The loader writes them from the proxy DLL's `DllMain`, after Windows has mapped the image and before the game's entry point runs, making each page writable for the write and restoring its protection afterwards. The caves execute but never store data - `.text` is mapped R-X; a future patch needing writable storage now has the loader's own memory for it (see 5).

---

## 3. Binary Reference Map

### 3a. Patched Sites and Reference Functions

The first two rows are **not patched** (see 1 and 4). They are listed because they remain useful landmarks when reading the binary.

| File Offset | VA | What |
|---|---|---|
| `0x23E665C` | - | AspectRatio constant (`3B 8E E3 3F`) in the exploration camera class constructor (function entry VA `0x1423E6F90`) |
| `0x69C8A8C` | - | Static photo projection float table: `DF 7C DB 3D 55 55 55 3F 39 8E E3 3F` (aspect at +8) |
| `0x440AAB0` | `0x14440B4B0` | `FMinimalViewInfo::CalculateProjectionMatrixGivenViewRectangle` (fragment; branch at `0x440ABC0`) |
| `0x44194C0` | `0x144419EC0` | `UCameraComponent::GetCameraView` true entry (flag merge at `0x441A14C`) |
| `0x4005B60` | `0x144006560` | `UCineCameraComponent::GetCameraView` (Super call at `0x4005B86`) |
| `0x4004910` | `0x144005310` | `UCineCameraComponent` constructor (filmback defaults 24.89/18.67, 50mm; `bConstrainAspectRatio=true` at `0x40049F6`) |
| `0x44003D0` | `0x144400DD0` | `ACineCameraActor` constructor |

Notes for anyone continuing this work:
- `.pdata` records here are heavily **chained** (hot/cold splits): resolve `UNW_FLAG_CHAININFO` to find true function entries, or caller scans come up empty.
- UObject constructors are reached via `InternalConstructor` thunks / inlining, not direct `E8` calls - map classes to constructors via the UTF-16 class-name strings passed to `GetPrivateStaticClassBody`.

### 3b. Aspect Ratio Float Constants
The binary uses two distinct "16:9" floats one bit apart - relevant when comparing aspect values exactly:
- `0x3FE38E3B` (`3B 8E E3 3F`, 1.7777779f) - camera-component constructor default;
- `0x3FE38E39` (`39 8E E3 3F`, 1.7777778f, closest float to 16/9) - `ACineCameraActor` constructor, the photo table, computed filmback ratios, and (field-verified) the cutscene cameras' serialized aspect.

Monitor-aspect values - cave A's gate bound is derived from this ratio:

| Resolution | Decimal | Hex (LE) |
|---|---|---|
| 2560x1080 / 5120x2160 | `2.3703703` | `26 B4 17 40` |
| 3440x1440 | `2.3888888` | `39 8E 18 40` |
| 3840x1600 | `2.4000000` | `9A 99 19 40` |
| 5120x1440 / 3840x1080 | `3.5555556` | `39 8E 63 40` |
| 1920x1080 (stock) | `1.7777778` | `3B 8E E3 3F` |

### 3c. Deck Nine Camera Class Landscape
The game does not use Sequencer CineCameras for cutscenes. Native classes found via UTF-16 string sweep: `UDUCameraComponent`, `UChronosCameraArmComponent`, `UD9CameraArmComponent`, `UD9VertigoCameraComponent`, `UChronosCameraColliderComponent`, `UChronosAICameraControlComponent`, `AChronosCameraPawn`, `AD9CameraPawn`, `UD9FreeroamCameraOffsetComponent`, `StandaloneCameraAnimationLinker`, and others. Multiple subclass constructors set `bConstrainAspectRatio` themselves (`or/mov byte [reg+0x2B4]` sites at `0x23E6648`, `0x43FEAFB`, `0x44004AB`, `0x2A519F5`, `0x2B82860`, `0x2C4959E`, ...); the plain `UCameraComponent` constructor does not. Deck Nine's Blueprints re-assert camera state per frame (`BP_MaxDefault_ChronosCameraArmComponent`, `BP_DefaultCameraStateTrigger`), which is why the fix operates on the per-frame view copy rather than component members.

---

## 4. The Photo Pipeline

UE compiles a static projection float table at `0x69C8A8C` (`DF 7C DB 3D 55 55 55 3F 39 8E E3 3F`, where the last dword is the 16:9 aspect). The photo render matrix samples this table rather than the live viewport.

This table and the player-camera constant at `0x23E665C` are both left **stock**, and the photo pipeline is correct precisely because of it: the capture matrix stays bit-identical to vanilla, and the square capture cameras (~1.0) sit below cave A's gate, so they keep their constraint and their classic projection. Field-verified - Polaroids and the in-game camera behave normally with both constants untouched.

The reasoning that says otherwise - "the photo view and the capture matrix have to agree, so patch both to the monitor ratio" - is wrong in a way that only measurement settles; section 10 has the capture, and section 6.2 the warning.

---

## 5. The Loading Side-Peek (RESOLVED)

During loading transitions, narrow strips of the still-streaming world could appear briefly at the screen sides.

Field triangulation across gate variants established that the loading view is **the destination scene's own cutscene-class camera, held behind a loading overlay that does not cover the extra width**. The wide camera renders correctly. Because that same camera renders the (wanted-wide) cutscene a moment later, no camera-identity or aspect gate can pillarbox loading without regressing cutscenes: it is a UI problem, not a camera problem.

The overlay widget is not the culprit either. `BP_LoadingWindow`'s `BlackScreen` image sits in a `CanvasPanelSlot` anchored `(0,0)-(1,1)` with offsets `(0,0,0,0)` - a full-viewport stretch - and `BP_TransitionWindow`'s `FullscreenImage` is identical. Both would cover an ultrawide viewport correctly. The 16:9 boxing is applied **upstream of the widgets**, at the UI host, and it hits every window - loading, main menu, pause, notifications - uniformly. One shared cause, not a per-asset defect.

**Resolved by the full-width UI patch (section 9c):** `BP_UIWindowManager`'s `WindowParent` is a fixed 3840x2160 centred box that clips every window to 16:9. Widening it to `2160 * monitorAspect` makes the overlay cover the side strips by itself, and puts right-anchored HUD elements on the physical screen edge - both symptoms close together. It ships as a mod container of our own rather than an executable patch; section 11 records the executable-only alternative for anyone who would rather not ship game data at all.

---

## 6. Dead Ends and Technical Pitfalls Explored

1. **Patching all 11 aspect constants to the monitor ratio ("full ultrawide")**: fills the screen but is Vert- - the fixed 16:9 horizontal FOV spans the wider frame, cropping ~20% of the vertical content (cut heads/chins in cinematics).
2. **Patching the player-camera constant and the photo table to the monitor ratio ("2-offset clean")**: correct photos and covered loading, but cutscenes stay pillarboxed and exploration goes Vert- (a constrained camera at monitor aspect crops vertically, felt as a zoom-in when leaving a cutscene). Worse, once the Hor+ branch is forced these two constants actively hurt: they desynchronise the dialogue hand-off, because the divisor must stay the aspect the FOV was *authored* for (2a, 10c). The shipped fix leaves both stock. This is the most tempting wrong turn in the whole binary - the addresses look exactly like the thing you want.
3. **Unconstraining every camera unconditionally**: produces perfect Hor+ cutscenes and exploration but breaks the photo pipeline (viewfinder/capture mismatch -> skewed Polaroids) and exposes streaming during loads. Constraint policy must be selective - hence the aspect gate.
4. **`Engine.ini` `AspectRatioAxisConstraint=AspectRatio_MaintainYFOV` alone**: ineffective - the axis constraint is only consulted on the *unconstrained* path (cutscene cameras are constrained), and the sequencer overrides/leaks the LocalPlayer value at runtime. Superseded by the branch patch (2b).
5. **38-byte NOP replacement of the flag-copy block**: crashed at boot (`EXCEPTION_ACCESS_VIOLATION`) - the replacement clobbered instruction boundaries. Only the single 7-byte `movzx` needs replacing; also, `[rdi+0x50]` in that function is `ProjectionMode`, not the axis constraint.
6. **C++ constructor-default patches for camera behavior** (CDO FOV values, `UCineCameraComponent::bConstrainAspectRatio` default): unreliable - cooked asset delta-serialization and Deck Nine's per-frame Blueprint camera logic re-supply values downstream of constructors. Patch the per-frame view copy instead.
7. **UE4SS Lua per-tick camera overrides**: a polling loop toggling `bConstrainAspectRatio`/axis constraint on components visibly fights the game's Blueprint camera system (flicker, lag, delayed aspect transitions). Runtime property wrestling is a dead end in this game; if a runtime component is ever reintroduced it must be event-scoped - driven by a load or dialogue event, writing once, not polling every tick.
8. **Class-identity gates for the loading view** (cine-only unconstrain, exact-16/9 float matching in either direction): field A/B testing proved the loading view shares camera identity and aspect constants with cutscene/menu cameras in every combination tried - see section 5 for why no such gate can work.
9. **Patching `FMinimalViewInfo::BlendViewInfo`'s flag merge** (`0x4408E19`): the function does lerp `AspectRatio` and merge `bConstrainAspectRatio` with `|=`, so a view-target blend really is constrained from weight 0 while the aspect is still 16:9. Rewriting the merge to `AND` was correct in itself and had **zero observable effect** - the constraint is re-asserted per frame from the component, so the blended value never survives. A good reminder that a real mechanism is not automatically the active one.
10. **Chasing the dialogue-exit zoom statically** (three iterations - intrinsic Hor+/Vert- framing difference, the blend flag merge above, then the patched `0x23E665C` constant as the ramp target). All three were wrong; see section 10d. The system is *animated*, and no amount of disassembly showed that. One runtime capture did.
11. **SUWSF in-memory patching for this fix**: SUWSF cannot express patch-time-computed code caves and rel32 displacements; its float-value patches also can't implement conditional logic. (Historical note: SUWSF's `Value="auto"` doesn't parse for floats; `Value="aspectratio"` works for plain constants.)

---

## 7. Reference Links

* **Lyall's UE ultrawide fixes:** [https://codeberg.org/Lyall](https://codeberg.org/Lyall) - SHfFix and TormentedSouls2Fix implement the same "clear bConstrainAspectRatio + force MaintainYFOV" pair as safetyhook mid-hooks; `UltrawidePatches` contains generic SUWSF patterns for simpler UE titles.
* **UE sequencer axis-constraint leak (zoom bug root cause):**
  [https://forums.unrealengine.com/t/4-24-onwards-aspectratioaxisconstraint-is-not-restored-if-a-camera-cut-occurs-between-two-identical-viewtargets/491662](https://forums.unrealengine.com/t/4-24-onwards-aspectratioaxisconstraint-is-not-restored-if-a-camera-cut-occurs-between-two-identical-viewtargets/491662)
  [https://forums.unrealengine.com/t/aspectratioaxisconstraint-is-resets-after-playing-sequencer/445043](https://forums.unrealengine.com/t/aspectratioaxisconstraint-is-resets-after-playing-sequencer/445043)
* **UE5 projection pipeline deep dive:** [https://80.lv/articles/deep-dive-ue5-camera-vs-scenecapture-maintain-axis-frustum-math-projection-pipeline](https://80.lv/articles/deep-dive-ue5-camera-vs-scenecapture-maintain-axis-frustum-math-projection-pipeline)
* **PCGamingWiki:** [https://www.pcgamingwiki.com/wiki/Life_Is_Strange:_Double_Exposure](https://www.pcgamingwiki.com/wiki/Life_Is_Strange:_Double_Exposure)
* **SUWSF:** [https://github.com/PhantomGamers/SUWSF](https://github.com/PhantomGamers/SUWSF)

---

## 8. Tools in This Package

1. **`crates/installer`** - `lis-ultrawide-fix`, the installer: one binary for Windows and one static musl binary for Linux, the loader below embedded in each, no dependencies beyond the standard library and nothing downloaded, ever. Four independent parts, the first three on by default: the ultrawide camera (it writes the embedded loader next to the game executable as `winhttp.dll`, and under Proton registers it in the prefix's `user.reg` as a per-application `DllOverrides` entry), the full-width UI mod container (section 9c-3), and two `Engine.ini` tweaks (chromatic aberration off, anti-blur TSR settings) written as one removable managed block. `restore` undoes all of it; `status` reports what is installed in lines a program can read; with no command it asks, at a terminal or through zenity, kdialog or yad, and without either it does nothing rather than install silently. Nothing it installs edits a shipped file, so there is nothing to back up; an executable that a version from before September 2026 patched in place is put back to stock from the `.original` backup those versions kept, once, and the backup is removed. It replaced a Python installer (`patcher.py`, in the repository's history up to 2026-09-02) whose on-disk results it reproduces exactly.
2. **`crates/loader`** - the loader, a Rust `cdylib` that the game loads as `winhttp.dll` (the game imports it, and Windows looks in the game's folder first). Not `version.dll`, the usual proxy name: Windows' compatibility shim engine (`apphelp.dll` + `AcGenral.dll`) is loaded before the game's imports are resolved whenever the player has set any compatibility option on the executable - the high-DPI override included, which is how this was found - and it imports `version.dll`, `userenv.dll` and `mpr.dll` from System32, so the game then reuses that copy and a proxy of that name is never loaded. No shim DLL imports or delay-loads winhttp. The loader forwards the ninety-one winhttp.dll exports to the System32 copy (each as sixteen machine words in and one out, which is correct for any integer-and-pointer signature on x64) and, in `DllMain`, does what sections 1 and 2 describe against the mapped image: the sites are tried at their known RVAs first and scanned for by signature otherwise, the caves are placed in `int3` padding, and every write is checked against its expected bytes before any page is touched - all or nothing, so a game the fix does not know runs unmodified. The display aspect comes from the primary display at launch, or from `LiSUltrawideCamera.ini` when the installer was given a resolution by hand. Every launch leaves `LiSUltrawideCamera.log` next to the DLL. The loader itself is only the Windows glue (`forward`, `runtime`, `win`); what it writes is decided in `crates/core` (`scan`, `pe`, `plan`, `games`), and `cargo test` runs that planner over a synthetic image and, on a machine with the game, over the stock executable, where its output must equal the six byte runs the old in-place patch wrote.
3. **`LiSUltrawidePatcher.exe` (& `.cs`)** - Windows GUI (WinForms; buildable with the stock .NET Framework `csc.exe`, the exact command in the `.cs` header). Only the source is in the repository: `.github/workflows/build.yml` compiles the exe on every push to main and publishes it, so the binary a user downloads is always the one this source produces and cannot quietly drift from it - the failure mode an earlier committed exe made invisible. A **thin front-end only**: it asks the installer where the game is (`lis-ultrawide-fix find`), detects the display, shows the four options as checkboxes, and runs `lis-ultrawide-fix.exe install` or `restore` from its own folder, streaming the output; the two badges under the path (the loader: installed, outdated, another program's winhttp.dll, not installed, and the loader's own verdict from the last launch read from its log; the UI container: current, stale after a game update, incomplete) are `lis-ultrawide-fix status` run in the background. When the installer reports a permission problem it offers to run the same command again as administrator. It contains no patch logic and no search of its own. An earlier version reimplemented the byte patches in C# and silently drifted out of sync - keeping one implementation is deliberate.
4. **`crates/core`** - everything the other two decide with, portable and tested on Linux and Windows: the camera patch planner (`plan`, `scan`, `pe`, `camera`), the per-game descriptors (`games`: names, ids, paths, the planner and the UI edits of each game), the IoStore and Zen readers (`iostore`, `zen`, `unver`), the container writer (`iostore`, section 12), the UI layout patcher (`ui_layout`, section 9c-3), the Kraken decoder (`kraken`, section 9e), the three digests the formats need written out in full (`hash`: SHA-1 for the stub `.pak`, SHA-256 for the game-build fingerprint and the container id, BLAKE3 for chunk metadata), the managed `Engine.ini` block (`engine_ini`), Steam libraries and Proton prefixes (`steam`, `wine`), the game search (`locate`) and display detection (`display`). The one build-time dependency is `winresource`, for the Windows version resources.
5. **`tools/make_icon.py`** - regenerates `LiSUltrawidePatcher.ico` (7 sizes, 16-256 px) with no imaging dependencies: shapes are supersampled by hand and written as PNG-compressed ICO entries via `zlib` and `struct`. The only Python left, for a committed asset that never changes.

### 8a. Where the Installer Looks for the Game

The installer does not care where it is run from - `locate` in `crates/core` looks in this order, and the GUI asks it (`lis-ultrawide-fix find`) rather than searching on its own:

1. next to the installer itself, and next to the current folder (`Chronos/Binaries/Win64/`, up to two levels up);
2. **every Steam library on the machine** - the Steam installations named in the registry and the usual `Program Files` locations, then each library folder listed in their `libraryfolders.vdf`, with the game's folder name read from `appmanifest_1874000.acf`;
3. the **Epic Games Launcher** manifests in `%PROGRAMDATA%\Epic\EpicGamesLauncher\Data\Manifests`;
4. the usual game roots on every fixed drive (`Games`, `Program Files`, `Program Files (x86)`, `GOG Games`, `Epic Games`, `SteamLibrary\steamapps\common`), including any folder whose name looks like the game's.

Nothing is ever scanned recursively, so this costs a few milliseconds. On Linux, the Steam Deck and macOS the same search covers `~/.steam`, `~/.local/share/Steam` and the Flatpak Steam data folder, which is where Proton installs live. The tool reports which of the four found it, and you can always override the result - **Browse** in the GUI, `--exe` on the command line.

---

## 9. Asset Pipeline and the UI Layer

Everything in this section was verified on 2026-08-30 by reading the shipped containers directly. The reader used is in `crates/core` (`iostore`, `zen`, with the Kraken decoder in `kraken`); it was first written in Python, and the Rust port was checked against that to the last byte.

### 9a. The Containers Are Not Encrypted

Section 5 previously stated that the IoStore directory index is encrypted and that asset paths are unrecoverable without an AES key. **That is wrong.** Decoded `FIoStoreTocHeader` for `pakchunk0-Windows.utoc`:

```
Version                5
TocEntryCount          56411
CompressionBlockCount  675076        CompressionMethod = Oodle
DirectoryIndexSize     2550433
ContainerFlags         0x09  = Compressed(0x01) | Indexed(0x08)
                             ... Encrypted(0x02) NOT set, Signed(0x04) NOT set
EncryptionKeyGuid      00000000-0000-0000-0000-000000000000
```

The directory index parses as plaintext with no key: **50,983 asset paths from `pakchunk0`, 8,843 from `pakchunk1`**. `pakchunk0-Windows.pak` (UE pak version 11) likewise carries a plaintext index and holds the cooked `.ini` config.

The real obstacle was never encryption - it is that UE 5.2 links Oodle statically into the shipping exe, so no `oo2core_*.dll` ships with the game. The fix's own Kraken decoder (`crates/core/src/kraken.rs`, section 9e) reads every compressed block, and everything opens. Containers are also unsigned, and UE mounts `.pak`/`.utoc` recursively under `Content/Paks`, so loose asset mods mount natively - this install already carries one (`Content/Paks/Mods/PSButtons-3161.*`).

Cooked packages use unversioned property serialization (`PackageFlags=0x80002200` = `FilterEditorOnly|UnversionedProperties|Cooked`, `bHasVersioningInfo=0`), so *general* property editing needs a `.usmap` (UE4SS `dumpusmap` or Dumper-7). Package structure - name map, imports, exports, class names, outer chains - decodes without one, and so do individual structs whose schema order is known.

### 9b. Verified Widget Layout

`BP_LoadingWindow` (`Chronos/Content/UI/BP/Window/`), full export map: root `D9CanvasPanel` under `WidgetTree`, children `BlackScreen` (`UMG.Image`) and `LoadingVisuals` (`D9CanvasPanel`), the latter holding `D9Image`, `D9RichTextBlock` and `AnimatedImage`. **No `ScaleBox`, `SizeBox` or `SafeZone` anywhere.** Decoded `UCanvasPanelSlot` layout data:

| Parent | Child | Anchors | Offsets | Alignment |
|---|---|---|---|---|
| `D9CanvasPanel` | `BlackScreen` | (0,0)-(1,1) | 0,0,0,0 | 0,0 |
| `D9CanvasPanel` | `LoadingVisuals` | (0,0)-(1,1) | 0,0,0,0 | 0,0 |
| `LoadingVisuals` | `D9Image` | (0,0)-(1,1) | 0,0,0,0 | 0,0 |
| `LoadingVisuals` | `D9RichTextBlock` | (0,1)-(0,1) | 374,-174,100,100 | 0,1 |
| `LoadingVisuals` | `AnimatedImage` | (0,1)-(0,1) | 220,-140,97,136 | 0,1 |

The black plate is a full-viewport stretch. `BP_TransitionWindow`'s `FullscreenImage` is the same. Neither asset can be the reason the sides are exposed.

`BP_NotificationWindow` is equally innocent: `MainPanel -> NotificationPanel` at anchors `(1,0)-(1,0)`, alignment `(1,0)` - anchored and aligned to the **right edge of the viewport**, not to any fixed coordinate. `BP_Notification_SMS` likewise pins its content with `(1,*)` anchors. If the UI host spanned the viewport, these would already sit on the physical right edge at any aspect ratio.

`BP_PauseWindow` fixes the design canvas: `MainButtons -> Pause` carries offsets `(1920, 590, 300, 80)` at anchor `(0,0)` with alignment `(0.5,0)`, and `VignetteTop`/`VignetteBottom` split at `Y=1080`. **The UI is authored on a 3840x2160 canvas** (1920 = its horizontal centre, 1080 = its vertical centre).

### 9c. Where the 16:9 Boxing Comes From - SOLVED

`Chronos/Config/DefaultEngine.ini` (extracted from `pakchunk0-Windows.pak`):

```ini
[/Script/Engine.UserInterfaceSettings]
UIScaleRule=ScaleToFit
DesignScreenSize=(X=3840,Y=2160)
UIScaleCurve=(... ExternalCurve=CurveFloat "/Game/UI/Data/FC_UI_DPI_Scale.FC_UI_DPI_Scale")
```

`FC_UI_DPI_Scale` decodes to keys `(0, 0.25) (620, 0.5) (1080, 0.5, constant) (1440, 1.0) (2160, 1.0)`.

This is **not** the cause. `EUIScalingRule::ScaleToFit` computes `min(W/3840, H/2160)` and ignores the curve; at 5120x2160 that is `min(1.333, 1.0) = 1.0`, confirmed at runtime (`viewportSize=5120x2160  dpiScale=1.0`). A DPI scale of 1.0 makes UMG's design space equal the real viewport, so config overrides cannot move anything - and because the rule is height-bound, the scale is 1.0 at *every* aspect wider than 16:9. Config is ruled out. So is `UChronosUISceneTextureSubsystem`, whose entire reflected surface is a single `MaterialParamColl` object - it feeds material parameters, it does not composite the UI through a render target. A keyword sweep of the whole `.usmap` also confirms **no Deck Nine class has any aspect, safe-zone, letterbox or design-size property**.

The cause is a single hardcoded slot in `Chronos/Content/UI/BP/BP_UIWindowManager.uasset`:

```
WidgetTree.RootWidget = ScaleBox        (Stretch not serialized -> CDO default, ScaleToFit)
  \- WindowManager (UMG.CanvasPanel)
       |- GameStyle
       |- WindowParent (D9Runtime.D9CanvasPanel)
       |       anchors = (0.5,0.5)-(0.5,0.5)   alignment = (0.5,0.5)
       |       offsets = (0, 0, 3840, 2160)             <-- THE BUG
       |    |- InputBlocker    anchors (0,0)-(1,1) offsets 0        full stretch
       |    |- SharedBGPanel   anchors (0,0)-(1,1) offsets 0        full stretch
       |    |    \- Darken / BackgroundBlur / ModalDarkenBG        full stretch
       |    \- BuildInfoLabel anchors (1,0)-(1,0) alignment (1,0)
       \- WatermarkLabel
```

`WindowParent` is the `D9UIWindowManager::WindowParent` member - the panel **every** game window is reparented into at runtime. A point-anchored slot with alignment `(0.5,0.5)` and offsets `(0,0,3840,2160)` is a **fixed 3840x2160 box centred in the viewport**. At 5120x2160 with DPI scale 1.0 that leaves 640 px of dead space on each side, and every window inside it - loading, transition, main menu, pause, notifications - is clipped to exactly 16:9 no matter how correctly its own slots are anchored. One slot explains every symptom.

The root `ScaleBox` reinforces it: with the default `ScaleToFit` it scales its content by the content's own desired size, which `WindowParent`'s fixed 3840x2160 supplies.

### 9c-1. The Fix and Its Trade-off

The design space is always `2160 * aspect` wide by 2160 tall for any aspect at or above 16:9 (the `ScaleToFit` DPI rule is height-bound). So the minimal, mechanism-preserving change is **one float** in `WindowParent`'s `CanvasPanelSlot`:

```
offsets.Right : 3840.0  ->  2160 * monitorAspect
                            5120  @ 21.33:9      5160  @ 3440x1440
                            7680  @ 32:9         6912  @ 32:10
```

That keeps the existing centred-box mechanism intact and simply sizes the box to the real design space, so `ScaleToFit` resolves to scale 1.0 filling the viewport exactly. Full-stretch children (the loading `BlackScreen`, `SharedBGPanel`) then cover the whole screen, and edge-anchored children (`NotificationPanel` at `(1,0)`, `BuildInfoLabel` at `(1,0)`) land on the physical screen edge.

**Trade-off to test:** widening `WindowParent` moves its local origin 640 px left, so any descendant positioned by *absolute offsets* rather than fractional anchors shifts with it. `BP_PauseWindow`'s `MainButtons -> Pause` is a known example - anchor `(0,0)`, offsets `(1920, 590, 300, 80)`, alignment `(0.5,0)` - i.e. centred by hardcoding half of 3840. After the change it would sit 640 px left of centre. Elements using fractional anchors (the great majority, including `MainButtons -> D9VerticalBox` at `(0.5,0.5)`) are unaffected. The blast radius is enumerable statically: scan every `BP_*Window` for point-anchored slots whose offsets cluster near 1920/3840 and adjust that handful.

### 9c-2. Blast-Radius Audit

An audit script (`tools/assetdump/audit_offsets.py`, in the repository's history before the move to Rust) walked every widget package under `Chronos/Content/UI/`, decoded each `UCanvasPanelSlot`, resolved the in-package ancestor chain, and reported the slots whose horizontal placement is tied to the 3840 canvas. Result over **197 packages / 1847 slots**, for a 5120x2160 target (origin shifts 640 px left):

| Class | Count | Meaning | Action |
|---|---|---|---|
| `HIGH` | 1 | centred by hardcoding half of 3840 | fix |
| `FIXW` | 6 | fixed 3840-wide and *not* centred - stops spanning the widened parent | fix |
| `MED` | 108 | positioned by an absolute X on the 3840 canvas | mostly covered by one upstream change, see below |
| `BOX` | 31 | a deliberate, *centred* 3840x2160 box | none - stays a 16:9 island exactly as today |

The `BOX` class is the key discovery: **the centred 3840x2160 slot is a recurring Deck Nine idiom**, used 38 times across the UI (`BP_SettingsWindow`, all seven `BP_KeybindingSettings*`, the `BP_PlayerChoices*` family, `BP_PhotographyWindow` backgrounds, and three of the six player-menu tabs). Those screens are self-protecting: widening the master `WindowParent` leaves them centred 16:9 islands, i.e. visually unchanged.

The 7 slots that genuinely need a value change:

```
[HIGH] BP_PauseWindow             MainButtons -> Pause        anchor(0,*) align(0.5,*) offsets=(1920,590,300,80)
[FIXW] BP_SettingsWindow          MainPanel   -> Background                 fixed 3840x2162 at (0,0)
[FIXW] BP_SaveSelectWindow        MainPanel   -> D9Image                    fixed 3840x2160 at (0,0)
[FIXW] BP_SquareEnixAccountWindow MainPanel   -> CanvasPanel_Background     fixed 3840x2162 at (0,0)
[FIXW] BP_SquareEnixAccountWindow MainPanel   -> WidgetSwitcher_CurrentView fixed 3840x2162 at (0,0)
[FIXW] BP_UISettings              MainPanel   -> Buttons                    fixed 3840x2160 at (0,0)
[FIXW] BP_CollectiblePosterUI     ReadPanel   -> D9Image                    fixed 3840x2160 at (0,0)
[FIXW] BP_ShiftChoiceUI           ChoicesMenu -> ChoiceButton               fixed 3840x2160 at (0,1)
```

Each is a one-value edit: convert `HIGH` to a fractional anchor (`anchorX 0 -> 0.5`, `offsets.Left 1920 -> 0`), and convert each `FIXW` to a horizontal stretch (`anchorMax.X 0 -> 1`, width offset -> 0).

**93 of the 108 `MED` findings live under `BP/Controls/PlayerMenu/`** - journal pages and menu tabs. `BP_PlayerMenuWindow`'s `ContentPanel` already boxes three of its six tabs (`ObjectivesTabUI`, `CluesTabUI`, `SocialMediaTabUI` at the centred 3840x2160 idiom) while `JournalTabUI`, `SMSTabUI` and `CollectiblesTabUI` are full-stretch. Giving those three the same centred box their siblings already have is **three slot edits that neutralise the whole 93** - and it matches the game's own convention rather than inventing one.

That leaves roughly a dozen individually-placed `MED` slots (7 in `BP/Window`, 6 button controls, 2 player-choice controls) to eyeball in-game after the change.

**Caveat on the numbers:** `chain_preserves_width` resolves ancestors only *within* a package. A control whose host in another package is itself a fixed box does not move, so `MED` is an upper bound; the `BOX` inventory above is what resolves most of those cases by hand.

There is no exe-side equivalent - the value lives in cooked asset data, so this is the one part of the fix that cannot be a byte patch. See 9c-3 for how it is actually delivered.


### 9c-3. Implementation - `crates/core/src/ui_layout.rs`

Delivered as its own IoStore container in `Content/Paks/Mods/`, which the engine mounts after `pakchunk0` and which therefore shadows the ten packages it carries. The game's own files are only read, never written: Steam's Verify Integrity has nothing to repair, a game update cannot half-overwrite the fix, and `--restore` is three file deletions. The container is ~120 KB - the packages are copied verbatim apart from one float each - and it has to be generated on the user's machine either way, because the values written depend on the display (`2160 x aspect`).

Method, per modified package:

1. read and decompress the package chunk out of `pakchunk0`, decode the target `UCanvasPanelSlot` (located by the widget its `Content` points at, never by hardcoded offsets, so it survives game updates);
2. verify the current value matches the expected old value, and that the float occurs exactly once in the slot payload;
3. write the new float into a copy of the chunk;
4. carry the package's `FFilePackageStoreEntry` over from `pakchunk0`'s container header verbatim - export counts, imported package ids and shader map hashes are unchanged by a float edit, and the imports still resolve because they point at packages `pakchunk0` continues to serve;
5. publish the copies as a new container (section 12), uncompressed, with its own container header, directory index and perfect hash;
6. re-read every edited slot back through the container reader - out of the file the game will actually mount.

An earlier release edited `pakchunk0` in place instead, appending the modified chunks to the 18 GB `.ucas` and repointing its TOC. That worked, but it made Verify Integrity a ~20 GB re-download and needed backup fingerprinting to survive game updates. `undo_in_place_patch()` removes it on upgrade, which also guarantees the packages read above come from a stock container.

The one thing the move costs is what a game update now does. The in-place patch was simply overwritten by one - the fix vanished, harmlessly. A container is not: it keeps shadowing ten packages with copies cooked for a build that is gone, and cooked assets are tied to the build's global name map and script objects, so the failure is a broken UI rather than a missing feature. `container_state()` closes that by fingerprinting the game's `.ucas` when the container is built (size, and SHA-256 of the first megabyte) and comparing on every check; `lis-ultrawide-fix status` reports it as `files:` / `filesdetail:`, a plain run prints it under the executable line, and the GUI gives it a second badge. It reads a megabyte and needs no Oodle, so it answers even where the build step would be skipped. What it cannot do is fire at launch - nothing of this fix runs then, by design - so the warning lands the next time the installer is opened.

The 15 edits, all derived from the design space rather than hardcoded:

| Package | Slot | Field | Change |
|---|---|---|---|
| `BP_UIWindowManager` | `WindowParent` | Right | `3840 -> designW` |
| `BP_PauseWindow` | `Pause` | Left | `1920 -> designW/2` |
| `BP_SettingsWindow` | `Background` | Right | `3840 -> designW` |
| `BP_SaveSelectWindow` | `D9Image` | Right | `3840 -> designW` |
| `BP_SquareEnixAccountWindow` | `CanvasPanel_Background`, `WidgetSwitcher_CurrentView` | Right | `3840 -> designW` |
| `BP_UISettings` | `Buttons` | Right | `3840 -> designW` |
| `BP_CollectiblePosterUI` | `D9Image` | Right | `3840 -> designW` |
| `BP_ShiftChoiceUI` | `ChoiceButton` | Right | `3840 -> designW` |
| `BP_MainMenuWindow` | `MainButtons`, `D9Image`, `GamerTag` | Left | `+ (designW-3840)/2` |
| `BP_MainMenuWindow` | `InfocastPanel` | Left | `- (designW-3840)/2` |
| `BP_TitleWindow` | `GamerTag`, `PressAnyKey` | Left | `+ (designW-3840)/2` |

The main-menu and title screens are full-bleed compositions with no 3840 box to widen - their elements are inset from the box edges, so widening dragged them onto the physical screen edge. Re-inseting restores Deck Nine's authored framing; this is a deliberate choice to keep those two screens 16:9-composed rather than a limitation.

**Field-verified at 5120x2160**, both as an in-place edit and as the mod container: loading overlay covers the full width (side-peek gone), phone notifications sit on the physical right edge, pause title centred, main menu back to its authored 16:9 composition. The three full-stretch player-menu tabs (`JournalTabUI`, `SMSTabUI`, `CollectiblesTabUI`) are *not* patched - see section 11, which the move to a container of our own now makes tractable.

### 9d. Binary Reference - UI Classes

Native window classes recovered from the exe's UTF-16 string table: `UUIWindow`, `UOverlayUIWindow`, `UModalUIWindow`, `UUIObjectWindow`, `UUIWindowLogic`, `UIWindowProps`, `UD9UIWindowManager`, `UChronosUIWindowManager`, `UChronosUISceneTextureSubsystem`, plus one `UChronos*Window` / `U*Window` pair per screen (`UChronosLoadingWindow`, `UChronosNotificationWindow`, `UChronosMainMenuWindow`, `UChronosPauseWindow`, ...). `UScaleBox`, `USafeZone` and their slots are linked in but unused by the windows inspected so far.

### 9e. Reading Oodle-Compressed Packages

The game's data files are 97% Oodle-compressed - every compressed block in `pakchunk0-Windows.utoc` is Kraken (decoder type 6; `global.utoc` is stored uncompressed) - and Oodle ships *statically linked* inside the game executable, so there is no `oo2core_*.dll` next to the game to borrow. Oodle itself cannot be bundled: it is Epic's engine code under the Unreal Engine EULA, which permits redistribution only inside a product built with the engine. Earlier releases downloaded Epic's Oodle-for-UE build at install time instead, which is what most UE modding tools do, but a tool that downloads and runs a native library is a hard sell to antivirus heuristics and to mod-hosting rules alike, and it left native Linux with no decoder at all.

The fix reads the packages with its own Kraken decoder, **`crates/core/src/kraken.rs`**, a port of the Kraken parts of [ooz](https://github.com/powzix/ooz) (Copyright (C) 2016, Powzix, GPL-3.0-or-later - the same license as this project), by way of a pure-Python port that shipped first and that the Rust reproduces function for function. Only Kraken is implemented; Mermaid, Selkie, Leviathan, LZNA and Bitknit streams are rejected with a named error. Speed is beside the point: the fix decodes ten packages, 34 KB compressed, 120 KB out. What matters is that there is nothing to download, nothing native to load, and no platform difference - the same code runs on Windows and Linux.

It was verified against Epic's own decompressor byte for byte: every block of the ten packages the fix edits, a random sample of 1,500 of the 655,524 compressed blocks in `pakchunk0` (94 MB decoded), and a set of streams produced by Epic's compressor from non-game inputs at every level and the option variants that change the on-disk coding. That last set ships in `tests/kraken/` and runs on every push, on Linux and Windows. The compressor used to make it is Oodle 2.9.10, the build UE 5.2 ships: Epic's current builds emit only the classic Kraken codings, while the game's data is mostly the TANS, RLE and multi-array literal codings, the newer Huffman table coding and scaled offsets that 2.9.10 produces; The ignored test `decodes_every_sampled_block_of_the_games_containers` in `crates/core/tests/ui.rs` decodes a sample of the game's blocks, which is the check to run after a game update (the comparison against a native Oodle library was a Python-era script, `tools/assetdump/verify_kraken.py`, in the repository's history). Coverage of the decoder's code paths is measured from the decoder's own statistics, so a test run reports which block codings it actually exercised.

A native Oodle library can still be used by the research scripts - which read far more than the fix ever does - by pointing `LISDE_OODLE_DLL` at one. Nothing looks for a library on its own any more.

---

## 10. Runtime Camera Measurement - The Letterbox Ramp

Sections 2-9 were derived statically. The dialogue-exit zoom resisted that approach through three wrong hypotheses, so it was settled by measurement instead: a read-only UE4SS Lua mod sampling `APlayerCameraManager` every frame and logging `ViewTarget.Target`, `ViewTarget.POV.{FOV, AspectRatio, bConstrainAspectRatio, Location}` and `CameraCachePrivate.POV` (the finished, post-blend view that reaches the renderer). It hooked nothing and wrote nothing back. **Debug only - it is not part of the shipped fix.**

`FMinimalViewInfo` and `FTViewTarget` are fully reflected, so all of the above is readable from Lua. (`FGeometry`, used for the UI work in section 9, is not - hence that being done from static asset reads.)

### 10a. What a dialogue exit actually does

Compressed capture of one hand-off, with the vertical FOV derived as `2*atan(tan(FOV/2)/aspect)`:

```
rows      view target               aspect     con     FOV h            vFOV
348-352   BP_ChronosCameraPawn_C    1.77778    false   35.39 -> 65.08   20.35 -> 39.49
353-411   BP_ChronosCameraPawn_C    1.778 .. 2.370  false   65.09 -> 72.31   39.43 -> 34.27
412-481   BP_ChronosCameraPawn_C    2.37037    true    72.39 -> 75.00   34.32 -> 35.88
482-507   BP_Max_Coat01A_C          1.77778    false   75.00            46.69   <-- +10.8 deg snap
```

Three facts fall out of this and none of them were guessable:

1. **The camera's `AspectRatio` member is animated**, over roughly a second, from the authored 16:9 up to the **viewport** aspect. It is not a blend between two static cameras and it is not read from any constant we patch - the capture above was taken with both aspect constants at their stock 16:9 values and the ramp still ended at `2.37037` on a 5120x2160 display.
2. **The horizontal FOV is identical at both ends of the hand-off** (75.00 on the camera pawn, 75.00 on the gameplay camera). The entire visible artifact was the aspect.
3. **Free-roam exploration runs at `1.77778` unconstrained** - it was already rendering Hor+ through cave A, and had been since cave A was introduced. The documentation claiming "exploration constrained at monitor aspect" was wrong.

### 10b. Why it looked like two different bugs

The ramp is the game's **letterbox-open animation**: it widens the cinematic frame out to the screen when returning control. On a 16:9 display it is a no-op, because the authored aspect already equals the viewport aspect. In stock UE on an ultrawide display it is still harmless - a constrained camera takes the `MaintainXFOV` path, where `AspectRatio` only sizes the view *rect*.

Under the forced `MaintainYFOV` branch (2b), `AspectRatio` becomes the FOV **divisor**. The same animation then produced two symptoms that looked unrelated:

- while the ramp was inside cave A's old `(1.75, 1.8)` window the camera was unconstrained, and the climbing divisor narrowed the vertical FOV - **a zoom-in**;
- the moment the ramp crossed `1.8` the camera fell *out* of the window, the constraint came back at an aspect well below the viewport, and the view **pillarboxed**, the bars then shrinking to nothing as the ramp reached the viewport aspect;
- and when the gameplay camera finally took over at 16:9, the framing **snapped** by +10.8 degrees.

### 10c. The fix

Cave A now does two things for every camera it unconstrains: clears `bConstrainAspectRatio` as before, **and pins the view's `AspectRatio` to the authored `1.7777778`**, writing it into `[rdi+0x48]`. That is safe because `GetCameraView` copies the component's aspect into the output view immediately *before* the patched site:

```
0x441A143  mov eax, [rbx+0x2B0]     ; component AspectRatio (the animated value)
0x441A149  mov [rdi+0x48], eax      ; view.AspectRatio = it
0x441A14C  call caveA               ; rdi still points at the view
```

The gate's upper bound also moves from a fixed `1.8` to `display aspect * 1.002`, so the whole ramp - endpoint included - stays inside the window instead of falling out of it mid-animation.

This restores the rule already stated in 2a: **the divisor must be the aspect the FOV was authored for.** The letterbox animation moves `AspectRatio` away from that authored value; pinning puts it back. Cameras below the gate (square capture cameras, ~1.0) are untouched, which is why the photo pipeline is unaffected.

### 10d. Hypotheses this replaced

Recorded because each looked convincing and each was wrong - the sequence is a fair warning about static-only reasoning on an animated system.

1. *"It is the intrinsic Hor+ / Vert- framing difference between cinematic and exploration cameras."* Wrong: exploration was never Vert-.
2. *"`FMinimalViewInfo::BlendViewInfo` merges `bConstrainAspectRatio` with `|=`, so a blend is constrained at weight 0 while the aspect is still 16:9."* The mechanism is real - the OR is at `0x4408E19`, and `AspectRatio` genuinely is lerped there - but changing it to `AND` had **no observable effect**, because the constraint is re-asserted per frame from the component rather than surviving in the blended view. Not the cause.
3. *"The ramp converges on the constant we patched at `0x23E665C`."* Wrong: reverting both constants to stock left the ramp ending at the same `2.37037`. The target is the viewport aspect.

Only the fourth attempt - measuring instead of inferring - produced the answer, and it took a single capture to do it.

---

## 11. Possible Improvements

- **Player-menu tabs:** three full-stretch tabs (`JournalTabUI`, `SMSTabUI`, `CollectiblesTabUI`) are not repositioned by the UI patch (section 9c). Giving them the centred box their three siblings already have needs a *structural* package edit - adding serialized properties changes the package size. That was impossible while the fix repointed chunks inside `pakchunk0`; now that it writes a container of its own, package size is free and the only missing piece is a package serializer that can add properties and fix up the export offsets.
- **Per-shot aspect variants:** if a specific cinematic ever appears pillarboxed, its camera is authored at an aspect outside cave A's gate window (section 2c); the gate can be extended per report.
- **An executable-only route to the full-width UI (section 5):** the mod container could be replaced by a load-window flag - `UChronosLoadingWindow` is a *native* class, so its construction and destruction are an exact load signal rather than a heuristic, and the loader DLL has writable memory of its own for the flag. One flag byte, two small caves, and cave A consulting the flag. It is more reverse-engineering than the container edit, and it is the option to take if shipping a game-data change ever becomes undesirable.
- **Life is Strange: Reunion** (`Iris/Binaries/Win64/Iris-Win64-Shipping.exe`, 688 MB): ships with Denuvo, so only the in-memory route is possible there, and the loader's design already fits - the game imports `winhttp.dll`, its code is stored in plain form (same entropy and `int3` density as this game's `.text`), and the scan only has to walk its renamed sections instead of `.text`. Nothing else carries over: none of the three signatures exist in it, not even loosened to any struct offset (UE 5.5 or later - IoStore TOC version 8 against 5 here, Steamworks 1.57 against 1.53), and its containers use a newer TOC, header and package format than `container.py` writes. Supporting it means new signatures and caves for its camera code, the runtime measurements of section 10 repeated, and a container writer for the newer format.


---

## 12. Writing an IoStore Container - `crates/core/src/iostore.rs`

Shipping the UI fix as its own container (section 9c-3) means writing `.utoc` / `.ucas` / `.pak` that the engine accepts. Two parts of the format are not documented anywhere useful and were recovered from the shipped files instead. Both were then checked by rebuilding real data byte for byte, which is a much stronger test than "the game did not crash".

### 12a. The TOC's Perfect Hash

`FIoStoreTocHeader` carries a seed table (`TocChunkPerfectHashSeedsCount`, half the entry count) that turns a 12-byte `FIoChunkId` into an entry index without a search. The construction it implies:

```
hash(seed, id) = FNV-1, 64-bit, over the 12 bytes
                 h = seed ? seed : 0xcbf29ce484222325
                 h = (h * 0x100000001b3) ^ byte          -- multiply, then xor
seedIndex      = hash(0, id) % SeedCount
seed  < 0      -> slot = -seed - 1                       -- the slot, stored directly
seed  > 0      -> slot = hash(seed, id) % EntryCount
seed == 0      -> the bucket is empty; the chunk is not in this container
```

Two details cost time. It is FNV-**1**, not FNV-1a - multiply then xor, not the other way round - and the modulo is taken on the full 64-bit value even though the function returns a `uint32`; truncating first resolves most chunks and quietly misses a few.

Recovering it without source: the bucket-size distribution is the oracle. A container's seed table records, per bucket, whether it is empty (`0`), holds exactly one chunk (a negative seed) or more (a positive one). For 56,411 chunks over 28,206 buckets that is 3,863 empty and 7,558 singletons - and only the right hash reproduces both counts exactly, since any other hash lands near those Poisson figures but not on them. With the function fixed, the full lookup was confirmed for **every chunk of `pakchunk0`** (56,411) and every chunk of an unrelated third-party mod container.

Writing is the inverse, the usual CHD construction: bucket by the unseeded hash, place crowded buckets first with a seed that spreads them across free slots, drop singletons into what is left and record the slot directly. `build_perfect_hash()` re-runs the engine's lookup over the finished table before returning it.

### 12b. `FIoContainerHeader`, version 2

```
uint32 Magic 'IoCn' | uint32 Version=2 | uint64 ContainerId
TArray<FPackageId>  PackageIds            -- sorted; the loader binary-searches
TArray<uint8>       StoreEntries          -- 24 bytes each, then the array data
TArray<FPackageId>  OptionalSegmentPackageIds   (empty)
TArray<uint8>       OptionalSegmentStoreEntries (empty)
FPackageStoreNameMap RedirectsNameMap           (empty name batch)
TArray<...>          LocalizedPackages          (empty)
```

Each `FFilePackageStoreEntry` is `int32 ExportCount`, `int32 ExportBundleCount`, then two `{int32 Count, int32 OffsetToDataFromThis}` views - imported package ids and shader map hashes - whose offsets are measured **from the view itself**, not from the end of the pair, and whose data follows the fixed block. `parse_container_header()` / `build_container_header()` round-trip both `pakchunk0`'s header (35,054 packages, 2.0 MB) and a third-party mod's byte for byte.

The fix does not synthesize entries: it copies each package's entry out of `pakchunk0` unchanged, which is correct precisely because a float edit changes no count, no import and no hash.

### 12c. The Rest of the Container

* **Chunks** are written uncompressed (block method 0). Compression only buys size on a 120 KB container, and it keeps the writer independent of Oodle - which is still needed to *read* the game's packages, so this changes nothing for the user, only for the code.
* **Meta** is a BLAKE3 digest truncated to 20 bytes, zero-padded to 32, plus a flags byte (0 when the chunk is uncompressed).
* **Directory index** is the same tree the reader in `iostore.rs` walks, mounted at `../../../Chronos/Content/`.
* **The `.pak`** is a stub with an empty index: the engine discovers IoStore containers through pak mounting, so a `.utoc` without a `.pak` of the same name is never looked at. Ours is generated, and matches a working third-party mod's stub byte for byte apart from its path-hash seed and the SHA-1 that follows from it.
* **`_P`** on the file name is the engine's marker for a patch pak, which mounts after the shipped containers - that is what makes our copy of a package win over `pakchunk0`'s.

The test `reads_the_games_ui_packages_as_the_python_did` in `crates/core/tests/ui.rs` runs both checks against `pakchunk0` - it resolves every chunk through the perfect hash and rebuilds the container header to the last byte - and the reader tests in `iostore.rs` do the same for what the fix writes. The Python original, which is how the numbers above were produced, passed on `pakchunk0`, on `pakchunk1`, on a third-party mod, and on what the fix writes; on 2026-09-02 the Rust writer's three files were compared with the Python writer's for the same inputs and were identical.

### 12d. What the File Checks Cannot Prove

Everything above is verified against files, and the finished container is read back through our own reader before the installer reports success. Whether the running game *mounts* `Content/Paks/Mods/` and prefers our packages is the one claim no file check can settle. **Confirmed in game at 5120x2160**: the container mounts and its packages win over `pakchunk0`'s. It stays the thing to check first if the UI ever comes up 16:9 with all three files in place - most likely after a game update, which is what the staleness check in 9c-3 is for.

---

## 13. Life is Strange: Reunion

**Game:** *Life is Strange: Reunion* (project `Iris`, Steam app 2624870, install folder `LifeisStrangeReunion`)
**Engine:** Unreal Engine 5.5.4 (the string `Unreal Engine 5.5.4` is in the executable; the build stamp reads `UE5 Jan 17 2026 01:53:27`, `UE5-CL-0`)
**Binary:** `Iris/Binaries/Win64/Iris-Win64-Shipping.exe`, 688,115,200 bytes, Denuvo
**Status (2026-09-04):** the camera fix is implemented (`crates/core/src/games/reunion.rs`) and verified in the game: menu, cutscenes and exploration all render full width at 5120x2160 (13h). It is the branch edit and cave A only; cave B, which Double Exposure needs, boxes Reunion's cutscenes and is left out (13e). The full-width UI is implemented too (13i): the same fixed 3840x2160 `WindowParent` as Double Exposure's, delivered as a mod container in the game's own UE 5.5 formats. Sections 13a to 13f are read from the files on disk; 13g lists what is still open.

Two of the three Double Exposure changes transfer: the same two immediates in the projection function and the same seven-byte site in `UCameraComponent::GetCameraView`. The third, cave B on the cine component's single direct `Super` call, exists in the binary but must not be applied here (13e). What changed is the register allocation at each site, the structure offsets (5.5 grew `FMinimalViewInfo` by 20 bytes ahead of `AspectRatio`), and the layout of the executable, which Denuvo wraps.

### 13a. The Executable

Fifteen sections, none with the usual names, and no `.rsrc` (there is no version resource to read). The code the engine was compiled to is `.sdata`; `.debug$P` is Denuvo's own section, 494 MB, readable, writable and executable at once, and holds no engine code.

| Section | RVA | Size | Flags | What |
|---|---|---|---|---|
| `.sdata` | `0x1000` | 137 MB (`0x8292000`) | code, X | the engine and game code; RVA = file offset + 0xC00 |
| `.xtls` | `0x8293000` | 35 MB | R | read-only data (import address table at `0x8295xxx`, constants, strings) |
| `.00cfg` | `0xa60a000` | 8 MB | RW | writable data |
| `.sxdata` | `0xae15000` | 6.5 MB | R | exception data (`.pdata`), by size |
| `.debug$P` | `0xb7e4000` | 494 MB | XRW | Denuvo |
| `.xtext` | `0x28f9c000` | 228 KB | code, not X | |
| `.idata` | `0x28fed000` | 6.5 MB | R | import directory |

Two consequences for the loader:

* **Cave placement must be confined to `.sdata`.** The Double Exposure loader takes the first int3 run in any executable section; here that would be `.sdata` too, only because it comes first in the table. The Reunion planner asks for a cave in the section that holds the patched sites, so a cave can never land in Denuvo's section whatever the section order.
* **Import calls are not `call [IAT]`.** Denuvo rewrites them: not one `FF 15` instruction in `.sdata` targets the `winhttp`, `tanf` or `atanf` slots, although all are imported. Finding a function by "it calls `atanf`" does not work here; finding it by the fields it touches does (13d). The import *table* is intact: `WINHTTP.dll` is imported directly (`WinHttpOpen`, `WinHttpConnect`, `WinHttpOpenRequest`, `WinHttpSendRequest`, `WinHttpReceiveResponse`, `WinHttpQueryHeaders`, `WinHttpCloseHandle`, `WinHttpCrackUrl`, `WinHttpGetDefaultProxyConfiguration`, `WinHttpGetIEProxyConfigForCurrentUser`), all of which the loader forwards, so `winhttp.dll` stays the proxy name. `dwmapi`, `winmm` and `version` are imported too and are ruled out for the reasons in section 8.

### 13b. The Containers

`Iris/Content/Paks` holds `global.utoc/.ucas` (770 bytes and 3 MB: one chunk, the script objects, uncompressed) and one `pakchunk0-Windows` set: `.utoc` 18.6 MB, `.ucas` 25.1 GB, `.pak` 2.1 GB. The `.pak` is not a stub as Double Exposure's is; what it carries is not yet known and does not matter for the UI fix, which only needs the IoStore side.

`pakchunk0-Windows.utoc` header, read with the field order of `FIoStoreTocHeader`:

| Field | Value |
|---|---|
| version | 8 (`ReplaceIoChunkHashWithIoHash`; Double Exposure is 5) |
| entries | 83,707 |
| compressed blocks | 867,741 (block size 64 KB) |
| compression methods | 1: `Oodle` |
| directory index | 4,172,140 bytes, mount point `../../../` |
| perfect hash seeds | 41,854; chunks without a hash: 0 |
| container id | `0x81e4dd3d9595e447` |
| flags | 9 = Compressed, Indexed (not encrypted, not signed) |
| chunk types | 1 (ExportBundleData) 59,688; 2 (BulkData) 19,190; 6 (ContainerHeader) 1; 8 (ShaderCode?) 4; 9 (ScriptObjects/other) 4,824 |
| per-entry meta | 24 bytes each (2,008,968 bytes for 83,707 entries): the 20-byte `FIoHash` plus flags, padded |

**Every compressed block is Kraken.** 3,899 blocks were sampled evenly across the 867,741 (every 222nd): the first byte is `0x8C` and the decoder type byte is 6 (Kraken) in all of them; no Mermaid, Selkie or Leviathan. `kraken.rs` decodes them as it is; `oozextract` is not needed (plan.md section 4).

The directory index names the UI packages the Double Exposure fix edits, under new names: `BP_IrisUIWindowManager.uasset` (Double Exposure: `BP_UIWindowManager`), `BP_PauseWindow`, `BP_SettingsWindow`, `BP_MainMenuWindow`, `BP_TitleWindow`, `BP_UISettings`, `BP_PlayerMenuWindow`, 532 `*Window*` names in all. Whether Reunion's UI is boxed to 16:9 at all, and whether `BP_IrisUIWindowManager` has the same fixed 3840x2160 `WindowParent`, is a question for the container reader and the game (13g).

### 13c. Structure Layouts (UE 5.5.4, as compiled here)

Read off `UCameraComponent::GetCameraView` (13d), which copies the component into the output view field by field. `rbx` is the component, `rdi` the `FMinimalViewInfo&`.

| Member | `FMinimalViewInfo` (Reunion) | (Double Exposure) | `UCameraComponent` (Reunion) | (Double Exposure) |
|---|---|---|---|---|
| FOV | `+0x30` | `+0x30` | `+0x230` | `+0x2A0` |
| FirstPersonFOV / FirstPersonScale | `+0x38` / `+0x3C` | none | `+0x234` / `+0x238` | none |
| OrthoWidth | `+0x40` | `+0x38` | `+0x23C` | |
| bAutoCalculateOrthoPlanes, AutoPlaneShift | `+0x44`, `+0x48` | none | `+0x240`, `+0x244` | none |
| bUpdateOrthoPlanes, bUseCameraHeightAsViewTarget | `+0x4C`, `+0x4D` | none | `+0x250`, `+0x251` | none |
| OrthoNearClipPlane, OrthoFarClipPlane | `+0x50`, `+0x54` | | `+0x248`, `+0x24C` | |
| PerspectiveNearClipPlane | `+0x58` | `+0x44` | (cine: `+0xDBC`) | |
| **AspectRatio** | **`+0x5C`** | `+0x48` | **`+0x254`** | `+0x2B0` |
| AspectRatioAxisConstraint (`TOptional`: value, then IsSet) | `+0x60`, `+0x64` | | `+0x258` (value) | |
| **bitfield: bit 0 = bConstrainAspectRatio** | **`+0x68`** (bit 1 bUseFirstPersonParameters, bit 2 bUseFieldOfViewForLOD) | `+0x4C` | **`+0x259`** (bit 1 = bOverrideAspectRatioAxisConstraint, bit 2 = bUseFieldOfViewForLOD) | `+0x2B4` |
| ProjectionMode | `+0x6C` | `+0x50` | `+0x263` | `+0x2B5` |
| PostProcessBlendWeight, PostProcessSettings | `+0x70`, `+0x80` | | `+0x2D0`, `+0x300` | |
| Overscan (float, two bools) | `+0xB08` | none | `+0x25C`, `+0x260`, `+0x261` | none |
| CameraToViewTarget | `+0xB10` | none | | |
| component flags byte (bit 1 bUsePawnControlRotation, 2 first-person FOV, 3 first-person scale, 4 bUseAdditiveOffset) | | | `+0x262` | |
| AdditiveFOVOffset | | | `+0x2D4` | |

The three members the fix touches are the ones in bold: the view's `AspectRatio` (the FOV divisor under `MaintainYFOV`), the view's constraint bit, and the component's constraint bit that is copied into it.

### 13d. The Three Sites

All RVAs are in `.sdata`; VA = `0x140000000` + RVA; file offset = RVA - `0xC00`. Every signature below matches exactly once in the whole 688 MB file.

**`FMinimalViewInfo::CalculateProjectionMatrixGivenViewRectangle`**, RVA `0x3698AA0` (`rcx` = ViewInfo, `dl` = the caller's axis constraint, `r8` = the constrained rect, `r9` = `FSceneViewProjectionData`). Its caller `CalculateProjectionMatrixGivenView` is at `0x3698A40` and computes the rect first. `CalculateProjectionMatrix()` (the constrained case) is at `0x36968D0`. The axis branch:

```
0x3698BAF: 80 7F 64 00        cmp  byte [rdi+0x64], 0     ; ViewInfo.AspectRatioAxisConstraint.IsSet()
0x3698BB3: 74 04              je   +4
0x3698BB5: 0F B6 77 60        movzx esi, byte [rdi+0x60]  ; ... overrides the caller's value
           ...
0x3698BD4: 3B C1              cmp  eax, ecx               ; SizeX vs SizeY
0x3698BD6: 7E 06              jle  +6
0x3698BD8: 40 80 FE 02        cmp  sil, 2                 ; MajorAxisFOV?       <- 02 at +7 becomes FF
0x3698BDC: 74 2C              je   Vert- path
0x3698BDE: 40 80 FE 01        cmp  sil, 1                 ; MaintainXFOV?       <- 01 at +13 becomes FF
0x3698BE2: 74 26              je   Vert- path
0x3698BE4: ... MaintainYFOV path: dl = 0, YAxisMultiplier = SizeY/SizeX
0x3698C3E: 40 80 FD 01        cmp  bpl, 1                 ; ProjectionMode == Orthographic (bpl, loaded at entry), kept
```

Signature `3B C1 7E 06 40 80 FE 02 74 ?? 40 80 FE 01 74 ??`; the immediates sit at +7 and +13 (Double Exposure: +6 and +15, near jumps). Same semantics as 2b: with both compares unsatisfiable every perspective camera takes the `MaintainYFOV` path, and the sequencer's leaked `MaintainXFOV` is harmless.

The Hor+ conversion of 2a is still there, with one addition. At `0x3698D7E`: if `dl == 0` (not `bMaintainXFOV`) and `[rdi+0x5C]` (AspectRatio) is not zero **and a global bool is false** (`mov rax,[rip+..]; cmp byte [rax], dl`: the `r.UseLegacyMaintainYFOVViewMatrix` console variable, off by default), then `HalfXFOV = max(0.001, FOV) * PI/360`, `tan`, divide by AspectRatio, `atan` (the calls at `0x3698DAF` / `0x3698DB8` reach the CRT's `tanf` / `atanf` through thunks at `0x813DBxx`). So the rule of 2a holds unchanged: for an unconstrained camera the view's `AspectRatio` is the FOV divisor and must stay the authored value.

**`UCameraComponent::GetCameraView`**, RVA `0x36A5D40` (`rcx` = component, `xmm1` = DeltaTime, `r8` = view). After the pawn-rotation and additive-offset blocks, the copy:

```
0x36A686F: F3 0F 11 47 30           movss [rdi+0x30], xmm0         ; view.FOV (+ AdditiveFOVOffset if set)
0x36A6874: 8B 83 54 02 00 00        mov   eax, [rbx+0x254]         ; component AspectRatio
0x36A687A: 89 47 5C                 mov   [rdi+0x5C], eax          ; view.AspectRatio = it
0x36A687D: 0F B6 8B 59 02 00 00     movzx ecx, byte [rbx+0x259]    ; <- patched (7 bytes)
0x36A6884: 33 4F 68                 xor   ecx, [rdi+0x68]
0x36A6887: 83 E1 01                 and   ecx, 1                   ; bit 0 = bConstrainAspectRatio
0x36A688A: 33 4F 68                 xor   ecx, [rdi+0x68]
0x36A688D: 89 4F 68                 mov   [rdi+0x68], ecx
0x36A6890: 0F B6 93 59 02 00 00     movzx edx, byte [rbx+0x259]    ; then bit 2 the same way, via edx
           ...
0x36A68A1: 0F B6 83 63 02 00 00     movzx eax, byte [rbx+0x263]    ; ProjectionMode: eax is dead until here
```

Signature `0F B6 8B 59 02 00 00 33 4F 68 83 E1 01`. The differences from 2c: the merge goes through `ecx` (not `eax`) and ends in a `mov` rather than a `xor` into memory; `eax` still holds the component's aspect at the site and is not read again before it is reloaded, so the cave can use `eax` for the comparison, and it must clear bit 0 of `ecx`. Later in the function `[rdi+0x68]` is rewritten twice (bits 2 and 1, both derived from `ecx`), so a cleared bit 0 survives; the overscan helper at `0x3696180` that runs after the copy (`rcx` = view, `xmm1` = Overscan from `+0x25C`) rescales `FOV` and `OrthoWidth` only, so a pinned `AspectRatio` survives too.

**`UCineCameraComponent::GetCameraView`**, RVA `0x3320130`: `RecalcDerivedData` at `0x33209F0`, then the `Super` call at `0x3320156`, then the virtual `UpdateCameraLens` (`[vtable+0x630]`) and the near-clip and focus writes (`[rdi+0x58]`, `[rdi+0xA80]`, `[rdi+0xA88]`).

```
0x332013A: 0F 29 74 24 20     movaps [rsp+0x20], xmm6
0x332013F: 49 8B F8           mov  rdi, r8                 ; the view, callee-saved
0x3320142: 0F 28 F1           movaps xmm6, xmm1
0x3320145: 48 8B D9           mov  rbx, rcx
0x3320148: E8 ...             call RecalcDerivedData
0x332014D: 4C 8B C7           mov  r8, rdi
0x3320150: 0F 28 CE           movaps xmm1, xmm6
0x3320153: 48 8B CB           mov  rcx, rbx
0x3320156: E8 E5 5B 38 00     call 0x36A5D40               ; Super::GetCameraView  <- rerouted to cave B
```

Signature `0F 29 74 24 20 49 8B F8 0F 28 F1 48 8B D9 E8 ?? ?? ?? ?? 4C 8B C7 0F 28 CE 48 8B CB E8`, call at +28. The shorter Double Exposure signature (`E8 ?? ?? ?? ?? 4C 8B C7 0F 28 CE 48 8B CB E8`) matches three places in this build, hence the longer one. **This is the only direct call to `0x36A5D40` in `.sdata`** (every `E8` in the section was decoded and checked), as in Double Exposure: the other camera classes reach the base through the vtable.

### 13e. The Caves for Reunion

Cave A, the logic of 2c with the Reunion registers and offsets; `eax` is free (13d), `ecx` carries the flags byte:

```
0F B6 8B 59 02 00 00   movzx ecx, byte [rbx+0x259]   ; the original instruction
8B 83 54 02 00 00      mov   eax, [rbx+0x254]        ; component AspectRatio
3D 00 00 E0 3F         cmp   eax, 1.75f
76 11                  jbe   done
3D <display*1.002>     cmp   eax, upper bound
73 0A                  jae   done
83 E1 FE               and   ecx, -2                 ; clear bConstrainAspectRatio before the merge
C7 47 5C 39 8E E3 3F   mov   dword [rdi+0x5C], 1.7777778f   ; pin the divisor
C3                     ret                           ; 38 bytes
```

Site: `0F B6 8B 59 02 00 00` becomes `E8 <rel32> 66 90`. Four writes in all; the loader's log ends in `applied 4 writes`.

**No cave B.** The first build carried Double Exposure's cave B over (2d, with the flag dword at `+0x68`: `48 83 EC 28 / E8 <rel32> / 48 83 C4 28 / 80 4F 68 01 / C3`, on the `Super` call at `0x3320156`), on the assumption that cine cameras were loading views here too. In the game the menu and free roam were wide and every cutscene was narrow; with the same loader minus cave B, cutscenes were wide (13h). Reunion's cutscenes are cine cameras (`UD9CineCameraComponent`, 13f), so re-asserting the constraint on the cine `Super` call boxes exactly the views the fix exists for. Loading views have not shown a problem without it. The site and the bytes stay documented here in case a later build needs a narrower rule (a cave B that checks the component's class, say).

### 13f. Aspect Constants and Camera Classes

`3B 8E E3 3F` (1.7777779f, the component constructor default) occurs 9 times in `.sdata`, `39 8E E3 3F` (1.7777778f) twice; Double Exposure's photo projection table (`DF 7C DB 3D 55 55 55 3F 39 8E E3 3F`) is not in this build. None of them is written, as before (section 1).

The camera classes, from the UTF-16 class-name strings (228 names contain `Camera`): the Double Exposure set is still there (`UChronosCameraArmComponent`, `UD9CameraArmComponent`, `UD9VertigoCameraComponent`, `UChronosCameraColliderComponent`, `UChronosAICameraControlComponent`, `AChronosCameraPawn`, `AD9CameraPawn`), plus Reunion's own: `AIrisPlayerCameraManager`, `UIrisCameraOverrideComponent`, `UIrisCameraRigOverrideComponent`, `UD9CameraRigComponent`, and, new, a cine-camera family: **`UD9CineCameraComponent`**, **`AD9CineCameraActor`**, `UD9CineCameraNoiseShake`, `UCineSetupSpeakerCameraAlignNode`, `CineSetupCameraData`, `UCineSetupCameraMetadata`. Engine 5.5's Gameplay Cameras plugin (`UGameplayCameraComponent`, the `*CameraNode` classes) is compiled in as well. Double Exposure had no `D9CineCamera` classes: its cutscenes were its own camera arms (3c), and the cine component only drove loading views, which is what cave B was for. A `UD9CineCameraComponent` derives from `UCineCameraComponent`, whose `GetCameraView` is the one direct caller of the base (13d), so with cave B in place every view such a component produces is boxed. That is the reading of 13h's narrow cutscenes, and the test in 13g item 1.

### 13g. Still Open

1. The letterbox ramp of section 10, measured with a UE4SS probe: whether Reunion animates a camera's aspect at dialogue boundaries as Double Exposure does, and whether the divisor pin behaves there. No zoom or snap has been reported from the first play session, so this is a check, not a known problem.
2. Loading and transition views without cave B: if one ever shows stretched or misframed, that is where a class-aware cave B would go (13e).
3. Resolved in 13i: `BP_IrisUIWindowManager`'s `WindowParent` is the same fixed 3840x2160 box, and the UI part of the fix now covers Reunion. What is left is the in-game pass over the screens the audit changed beyond the loading overlay (13i lists them).
4. Whether the game reads the `Engine.ini` the installer writes at `%LOCALAPPDATA%\Iris\Saved\Config\Windows\Engine.ini` (the path follows the project name as Double Exposure's does; the game created nothing else under `Iris\Saved` on its first run, so the folder is the one it uses).

### 13h. First Run (2026-09-03)

`lis-ultrawide-fix install --yes --game reunion` put `winhttp.dll` next to the executable and wrote the `Engine.ini` block; the game was launched through Steam (`steam://rungameid/2624870`). The log appeared within ten seconds:

```
loaded as D:\...\LifeisStrangeReunion\Iris\Binaries\Win64\WINHTTP.dll
display 5120x2160 (the primary display)
gate upper bound 2.3751
image: base 0x140000000, timestamp 0x69d7108c, size 0x29654000
  Hor+ projection branch (CalculateProjectionMatrixGivenViewRectangle): at rva 0x3698bd4, where it was expected
  GetCameraView flag copy (cave A site): at rva 0x36a687d, where it was expected
  cine Super::GetCameraView call (cave B site): at rva 0x332013a, where it was expected
  cave A at rva 0xc1c005, cave B at rva 0xc03883
  wrote 1 bytes at rva 0x3698bdb ... wrote 4 bytes at rva 0x3320157
applied 6 writes - the ultrawide camera fix is active
```

So the proxy is loaded by name under Denuvo, `DllMain` runs before the game's code, the pages in `.sdata` accept the writes, and Denuvo's checks do not object to them: the game reached its menu, was played for a few minutes and was quit by hand. Both caves landed in `.sdata` (13a), at the first padding runs of 59 and 29 bytes.

Two observations from that run, neither a problem of the fix:

* The game's own settings page reported the resolution as 4096x1728 in borderless mode: the desktop size after Windows' 125% display scaling, which means the executable is not marked DPI-aware. Double Exposure on this machine has the `HIGHDPIAWARE` compatibility layer set for the same reason (section 8). The aspect is the same either way, so the loader's bound and the caves behave the same; only the pixel count differs.
* The settings page itself was laid out across the full width, wider than a centred 16:9 box would allow, so at least that part of Reunion's UI is not boxed the way Double Exposure's was (9c). 13g item 3 keeps the question open for the rest of the UI.

A second launch (the same install) was watched for 200 seconds with a screenshot every half minute or so while the owner played the opening; the process stayed up throughout. What the frames show, all at the 4096x1728 window:

* The main menu: the 3D scene fills the width, the title and the buttons sit on the left as authored. Full-width, no bars.
* A cutscene close-up (a face and jacket filling the frame): full width, no bars.
* Several dialogue and cutscene frames (a bedroom, a snowfield): the picture reaches the top, bottom and right edges, but a black band about 16% of the width sat on the left, and one frame had bands on both sides of different widths. The owner's summary of that session: **menu wide, cutscenes narrow, exploration wide.**

**Third launch, cave B off.** The same loader built without cave B (four writes) was installed and the owner replayed the opening: **cutscenes wide.** So the narrow cutscenes were cave B's doing, which fits the class landscape (13f): Reunion's cutscenes are `UD9CineCameraComponent` views, and cave B re-asserts the constraint on every cine view. The descriptor now has no cave B (13e). The asymmetric band seen with cave B in place was the game's own dialogue framing on top of a constrained view, and has not been seen since.

### 13i. The UI (2026-09-04)

Reunion's UI is boxed exactly as Double Exposure's is (9c): `BP_IrisUIWindowManager` has the same 26 exports, and its `WindowParent` slot is the same point-anchored `(0.5,0.5)` slot with offsets `(0, 0, 3840, 2160)`, the fixed 16:9 box every window is reparented into. The loading window (`BlackScreen` and `LoadingVisuals` full-stretch, the text and hourglass anchored to the bottom left) and the transition window (`FullscreenImage` full-stretch) are the same as 9b, so the loading side-peek of section 5 comes from the same one slot. The settings page looked full-width in 13h because its `Background` is a fixed 3840-wide image starting at the box's left edge and `TabMenu` stretches: the parts of it that reach past the box do so by accident, not by design.

**Reading the containers.** Three things changed between UE 5.2 and 5.5, all in `crates/core`:

* `FIoContainerHeader` is version 4. The store entries lost the export and bundle counts with version 3 (`FFilePackageStoreEntry` is 16 bytes: the two `{count, offset}` array views), and version 4 appended a soft-package-reference table after the redirects: a 4-byte bool, then the ids and per-package index views when it is set. The game's header has it unset (`bContainsSoftPackageReferences = 0`), so the mod header writes the same. The reader and writer take the version (`parse_container_header`, `build_container_header`); the header of `pakchunk0` rebuilds to the byte from what is parsed, as it did for version 2.
* `FZenPackageSummary` is the 5.3 layout: the graph data went, `DependencyBundleHeadersOffset` (+40), `DependencyBundleEntriesOffset` (+44) and `ImportedPackageNamesOffset` (+48) came, and the name batch moved from +44 to +52. The export payloads are no longer in export-bundle order: they sit at `HeaderSize + CookedSerialOffset`, in export-map order (the bundle order of this package puts the class first; the data does not). `ZenPackage::parse` takes a `Summary` (`Ue52` or `Ue53`), which the game descriptor supplies.
* TOC version 8 (`ReplaceIoChunkHashWithIoHash`): the per-chunk meta at the end of the `.utoc` is 24 bytes (20-byte hash, flags, 3 bytes of padding) instead of 33 (32-byte hash, flags). Nothing else in the header moved; `build_container` writes whichever the game uses.

`UCanvasPanelSlot` serializes as before (`LayoutData`, `bAutoSize`, `ZOrder`, `Parent`, `Content`; `FAnchorData` with float margins and double vectors), so `unver.rs` needed nothing. All 1,660 packages under `Iris/Content/UI` parse and all 2,112 canvas slots in them decode.

**The audit** (the 9c-2 walk, redone over Reunion's 1,660 UI packages) found the same picture: 37 deliberate centred 3840x2160 boxes (`BP_PlayerChoices*`, the photography windows, three of the player-menu tabs, `InputButtonBarOverride` in several windows), the `Pause` title centred by a hardcoded 1920, a set of fixed 3840-wide backgrounds anchored at `(0,0)` under full-stretch panels, the main-menu and title compositions inset by 220 from the box edge, and the journal pages positioned by absolute X (unchanged from section 11: they shift with `JournalTabUI`, which stretches; the poster reader under `ReadPanel` is the same idiom). New here: the reading views (`BP_FRPosterWindow`, `BP_ObjectInspectWindow`, `BP_ChloePhotoPosterUI`, `BP_IrisPhotoPosterUI`) put their scroll buttons at `Left = 1870` under a full-stretch `ReadPanel`, which is 1920 minus half their width - centred by absolute X, the same idiom as `Pause`. `BP_ShiftChoiceUI` does not exist in this game.

The 30 edits, in `reunion.rs`, all derived from the design space as in 9c-3:

| Package | Slot | Field | Change |
|---|---|---|---|
| `BP_IrisUIWindowManager` | `WindowParent` | Right | `3840 -> designW` |
| `BP_PauseWindow` | `Pause` | Left | `1920 -> designW/2` |
| `BP_SettingsWindow`, `BP_OutfitWindow` | `Background` | Right | `3840 -> designW` |
| `BP_SaveSelectWindow`, `BP_FRPosterWindow` | `D9Image` | Right | `3840 -> designW` |
| `BP_SquareEnixAccountWindow` | `CanvasPanel_Background`, `WidgetSwitcher_CurrentView` | Right | `3840 -> designW` |
| `BP_MontageWindow` | `Background` | Right | `3840 -> designW` |
| `BP_UISettings`, `BP_OutfitSettings` | `Buttons` | Right | `3840 -> designW` |
| `BP_CollectiblePosterUI`, `BP_ChloePhotoPosterUI`, `BP_IrisPhotoPosterUI` | `D9Image` | Right | `3840 -> designW` |
| `BP_FRPosterWindow`, `BP_ObjectInspectWindow`, `BP_ChloePhotoPosterUI`, `BP_IrisPhotoPosterUI` | `UpButton`, `DownButton` | Left | `+ (designW-3840)/2` |
| `BP_MainMenuWindow` | `MainButtons`, `D9Image`, `D9TextBlock`, `GamerTag` | Left | `+ (designW-3840)/2` |
| `BP_MainMenuWindow` | `InfocastPanel` | Left | `- (designW-3840)/2` |
| `BP_TitleWindow` | `GamerTag`, `PressAnyKey` | Left | `+ (designW-3840)/2` |

**Checked in the game (2026-09-04, 5120x2160 at the 4096x1728 window):** `install --yes --game reunion` published the 16 packages with 0 mismatches; the game started with the container mounted, the loader log ended in `applied 4 writes`, the main menu rendered exactly as stock (title and buttons at their authored inset, so the re-inset edits do what they should), and Continue showed a loading plate black across the full width with the scene loading behind it: no side-peek. The intro montage plays with no UI for well over two minutes after launch; Escape skips it to the menu. Not yet looked at: the pause, settings, reading and player-menu screens listed above.

The descriptor's `UiFix` names the formats (`toc_version: 8`, `container_header_version: 4`, `summary: Ue53`) and the `Iris/Content/` paths; the mount point is `../../../Iris/Content/`, the mod name stays `LiSUltrawideUI_P`. The UMG design size is taken to be 3840x2160, as the `WindowParent` box says; the cooked `DefaultEngine.ini` sits compressed in Reunion's 2 GB `.pak`, which the fix has no reader for, so the `ScaleToFit` rule of 9c is assumed rather than read. The test `reads_and_rewrites_reunions_ui_packages` pins the 16 packages (chunk, size, hash, exports, every slot the edits touch) and builds the container for 5120x2160 into a temporary folder, reads it back through the container reader, and checks that each package differs from stock in exactly the edited floats.
