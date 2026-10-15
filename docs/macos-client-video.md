# macOS client: decode, concealment and Metal rendering

Part of the macOS client reference (`gfn-macos-client-architecture.md`): reverse-engineered from `libBifrost2.dylib` and `libGeronimo.dylib` of v2.0.88.129.

## VideoToolbox Decoding and Reference Concealment

### Decoder Setup
- Implemented in `VTDecoder` wrapping `VTDecompressionSession`.
- Attributes: Hardware acceleration enabled, empty `kCVPixelBufferIOSurfacePropertiesKey`, `kCVPixelBufferMetalCompatibilityKey = YES`, and pixel format `420v` (or `x420` for 10-bit HDR).
- Does not use `kVTDecompressionPropertyKey_RealTime`.
- Execution is strictly **synchronous**: `VTDecompressionSessionDecodeFrame` is invoked with flags 0. The output callback runs inline on the decode thread, storing the resulting `CVImageBufferRef` directly into an output slot on the decoder object with zero intermediate copies.

### Seamless Resolution Switching
When the server adjusts stream resolution, GFN does not destroy or recreate the `VTDecompressionSession`. Instead, it configures a format description with a clean aperture rectangle. When resolution changes, the decoder session continues running, and the client updates the clean aperture dimensions (`ImageBufferSetCleanRect`). The Metal renderer reads the clean aperture and updates texture sampling coordinates accordingly.

### Bitstream Concealment and Recovery Gate
- If a frame or slice is missing, `NvBitstreamConcealer` synthesizes dummy skip-style frames into the bitstream, ensuring VideoToolbox's reference frame buffer remains continuous and preventing hardware decode error -12909.
- If VideoToolbox returns a decode error, GFN does not feed subsequent dependent P-frames:
  1. Increments consecutive failure counter (10 consecutive failures terminates session).
  2. Issues Reference Frame Invalidation (RFI, 0x0301) or IDR request (0x0302) to the server.
  3. Flushes the compressed queue back to the last valid keyframe.
  4. Activates a recovering gate that discards all incoming non-recovery frames until a valid recovery frame arrives.

## Metal Rendering and Post-Processing Filters

### Layer Configuration
- SDL Metal view rendering to physical backing pixels (`drawableSize = SDL_GL_GetDrawableSize`).
- Uses a 2-drawable swapchain on VRR / ProMotion displays (`maximumDrawableCount = 2`) gated by a matching dispatch semaphore to prevent queue buildup.
- Layer colorspace is left unmanaged (`nil`) for SDR, routing BT.709 color straight to display native gamut. HDR streams set `BGR10A2Unorm` with Apple's `CAEDRMetadata`.

### Filter Hierarchy
- **1:1 Scale Bypass**: When source dimensions match drawable dimensions, all sharpening and scaling passes are bypassed. A single fragment shader executes YUV-to-RGB color matrix transformation straight into the drawable.
- **Upscaling 1.0x to 2.0x**: Executes NVIDIA Image Scaling 1.0.x (`CS_NvScaler`) compute kernel at sharpness 0.27, 0.58, or 0.82.
- **Upscaling > 2.0x**: Executes native-resolution NIS sharpening (`CS_NvSharpen`), followed by a 9-tap Catmull-Rom bicubic upscale pass.
- **Enhanced (Sharpness Off)**: Executes 9-tap Catmull-Rom bicubic filtering directly.

### Automatic GPU-Time Benchmark Gate
The client benchmarks rendering cost over 100 frames by measuring the 95th percentile GPU execution time (`GPUEndTime - GPUStartTime`):
$$\text{Threshold} = 0.8 \times \left(\frac{1,000,000}{\text{MaxFPS}}\right) \mu\text{s}$$
At 120 FPS, the budget is 6,667 $\mu$s. If p95 GPU time exceeds this threshold, the renderer automatically ratchets down the filter fallback ladder:
$$\text{NIS Scaler} \longrightarrow \text{Catmull-Rom} \longrightarrow \text{Shepherd6} \longrightarrow \text{Shepherd3} \longrightarrow \text{Hardware Bilinear}$$

### Presentation Timing
Submitted command buffers present using:
- `presentDrawable:atTime:` when targeting VRR / ProMotion displays.
- `presentDrawable:afterMinimumDuration:` on fixed-refresh displays to ensure cinematic pacing without compositing queue bloat.
