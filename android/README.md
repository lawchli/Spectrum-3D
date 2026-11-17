# Spectrum 3D for Android

Two Gradle modules:

- `:spectrum` is the library (AAR): a Kotlin / OpenGL ES 3.0 view, its configuration and a PCM input. It has no Compose dependency and declares no permissions.
- `:app` is a Compose demo app around it. It shows how to handle the recording permission, the microphone, system audio capture and the foreground-service notification. It is not needed to use the library.

The library has the Tungsten, Mel and Green styles and the four drawing modes (Lit Surface, Ridgeline, Mesh, Solid). The Dadaspec style, the Mel frequency axis and the tilt are not ported yet.

## Build

Toolchain: JDK 17, Gradle 8.13 (through the wrapper), Android Gradle Plugin 8.13.0, Kotlin 2.1.20, SDK platform 36 and build-tools 36.0.0. Node.js 22 or later must be on the `PATH`: the build exports the shared shaders and look constants from `web/` with it, and installs no packages.

```sh
export SPECTRUM_ANDROID_HOME=/path/to/android-sdk   # or: bash scripts/android-sdk-link.sh /path/to/android-sdk
bash scripts/android-build.sh :spectrum:testDebugUnitTest :spectrum:assembleRelease :app:assembleDebug
```

`scripts/android-build.sh` picks a JDK 17 from `SPECTRUM_JAVA_HOME`, then `JAVA_HOME`. It keeps the Gradle and Android user homes inside the ignored `toolchains/` folder of the repository.

| Output | Path |
|---|---|
| Library | `android/spectrum/build/outputs/aar/spectrum-release.aar` |
| Demo app, debug | `android/app/build/outputs/apk/debug/app-debug.apk` |
| Demo app, release | `android/app/build/outputs/apk/release/` |

The release APK is unsigned unless you provide a keystore that lives outside the repository, through four environment variables: `SPECTRUM_RELEASE_STORE_FILE`, `SPECTRUM_RELEASE_STORE_PASSWORD`, `SPECTRUM_RELEASE_KEY_ALIAS`, `SPECTRUM_RELEASE_KEY_PASSWORD` (all four or none).

To check that the AAR works on its own, this builds the demo app against the built AAR instead of the source module:

```sh
bash scripts/android-build.sh :spectrum:assembleRelease
bash scripts/android-build.sh --init-script ../scripts/android-aar-check.gradle :app:assembleRelease
```

## Embed it in your app

In the same Gradle build use `implementation(project(":spectrum"))`. From another project, copy the release AAR into your app's `libs/` folder and add `implementation(files("libs/<the aar file>"))`. Use Kotlin 2.1.20 or a compatible version. The library needs Android 8.0 (API 26) and OpenGL ES 3.0. The AAR already contains the shaders and look data, so your project does not need Node.

The shortest View integration:

```kotlin
private lateinit var spectrum: SpectrumController
private lateinit var spectrumView: SpectrumView

override fun onCreate(savedInstanceState: Bundle?) {
    super.onCreate(savedInstanceState)
    spectrum = SpectrumController(this, SpectrumConfiguration(
        palette = "green",
        view = "perspective",
        rowRate = 120,
        bandsPerSemitone = 2,
    ))
    spectrumView = SpectrumView(this, spectrum)
    setContentView(spectrumView)
    spectrum.startPcm(sampleRate = 48000)
}

// Push Float32 PCM from your own player, decoder or capture callback.
// It may be called on the thread that produces the audio.
fun onAudioBlock(interleavedStereo: FloatArray) {
    spectrum.pushPcm(interleavedStereo, channels = 2)
}

override fun onResume() { super.onResume(); spectrumView.onResume() }
override fun onPause() { spectrumView.onPause(); super.onPause() }
override fun onDestroy() { spectrum.dispose(); super.onDestroy() }
```

`SpectrumView` draws the axis labels and the strongest-peak readout itself and handles one-finger rotation, two-finger zoom and pan, and double-tap to reset. `SpectrumView.onResume` and `onPause` only manage drawing; your app calls them.

`samples` is 0 (the default) or 4. If 4× multisampling is not available it falls back to 0×. The default is 0× because on the one phone this was tested on (Sony XZ1 Compact, Adreno 540) 4× made the GPU driver fail and the process was killed as soon as the picture got wide. Test 4× on your target devices before turning it on.

In a Compose app use the same view, the library does not bring a second Compose UI:

```kotlin
@Composable
fun EmbeddedSpectrum(controller: SpectrumController) {
    val context = LocalContext.current
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    val view = remember(controller) { SpectrumView(context, controller) }
    DisposableEffect(view, lifecycle) {
        val observer = LifecycleEventObserver { _, event ->
            when (event) {
                Lifecycle.Event.ON_RESUME -> view.onResume()
                Lifecycle.Event.ON_PAUSE -> view.onPause()
                else -> Unit
            }
        }
        lifecycle.addObserver(observer)
        if (lifecycle.currentState.isAtLeast(Lifecycle.State.RESUMED)) view.onResume()
        onDispose { lifecycle.removeObserver(observer); view.onPause() }
    }
    AndroidView(factory = { view }, modifier = Modifier.fillMaxSize())
}
```

Whoever owns the controller (an Activity, a ViewModel, or an outer `DisposableEffect`) calls `controller.dispose()` when it is finally destroyed.

Change the row rate, bands per semitone, layers, time span, style and hold options with `configure(SpectrumConfiguration(...))`. Shortcuts: `chooseView`, `choosePalette`, `freeze`, `clearMaximum`, `stop`. `availableViews`, `availableStyles` and `availablePalettes` list what the shared look supports. A custom control panel can reach the full state through `model`, which is what the demo app does.

## PCM input

- `sampleRate` from 8000 to 192000 Hz. The analyser places frequencies and paces its rows by the input sample rate, not by the display refresh.
- Samples are Float32, normally within −1…1. Several channels are averaged. Non-finite samples count as zero.
- The buffer you pass is copied, so you can reuse it at once.
- The queue holds at most 16 blocks of 1024 frames. When it backs up, the oldest block is dropped so that latency cannot grow without bound; `droppedPcmBlocks` tells you when that happened.
- Push in real time, by the audio clock. It is not made for feeding a whole file at once.

## Capture is opt-in

A new controller or view stays at `source == "off"`: it opens no microphone, shows no permission dialog and starts no notification. `startTestSignal()` gives you a picture without any permission.

If you already have an audio path, use `startPcm` and `pushPcm`. The library also offers `startMicrophone(deviceId)` and `startPlaybackCapture(projection)`. Before calling them your app must hold `RECORD_AUDIO`, and for system audio capture (Android 10 or later) the MediaProjection grant and a foreground service. `CaptureService.kt` in the demo app shows that part; the service and the notification are not merged into your app by the AAR.

## How it is put together

The GLSL shaders, the look constants and the interface strings have one source, the `web/` folder. `scripts/export-android.mjs` writes them into the library's `assets/spectrum/` namespace before every build.

The analysis is the same as in the web version: Blackman-windowed FFTs of 8192, 2048 and 1024 points, 60, 120, 240 or 480 rows per second, 1, 2, 4 or 8 bands per semitone, and a six-level power history. The unit tests compare it with 80 reference scenes produced by the JavaScript implementation (`tests/fixtures/`).
