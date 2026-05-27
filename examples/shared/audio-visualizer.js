/**
 * Real-time waveform + frequency visualization for WebRTC audio tracks.
 *
 * Uses Web Audio AnalyserNode and a canvas. Supports HTMLAudioElement playback
 * or a raw MediaStream (e.g. microphone preview).
 */

/**
 * @typedef {Object} AudioVisualizerOptions
 * @property {HTMLCanvasElement} canvas
 * @property {HTMLAudioElement} [audioElement]
 * @property {MediaStream} [mediaStream]
 * @property {string} [waveColor]
 * @property {string} [barColor]
 */

/**
 * @typedef {Object} AudioVisualizer
 * @property {() => void} stop
 * @property {() => void} resume
 */

/**
 * Attaches a live oscilloscope + spectrum graph to an audio source.
 *
 * @param {AudioVisualizerOptions} options
 * @returns {AudioVisualizer}
 */
export function attachAudioVisualizer(options) {
  const {
    canvas,
    audioElement,
    mediaStream,
    waveColor = '#38bdf8',
    barColor = '#818cf8',
  } = options

  if (!audioElement && !mediaStream) {
    throw new Error('attachAudioVisualizer requires audioElement or mediaStream')
  }

  const ctx = canvas.getContext('2d')
  if (!ctx) {
    throw new Error('canvas 2d context unavailable')
  }

  const audioCtx = new AudioContext()
  const analyser = audioCtx.createAnalyser()
  analyser.fftSize = 2048
  analyser.smoothingTimeConstant = 0.75

  // Prefer MediaStream tap: works reliably with WebRTC remote tracks while the
  // <audio> element plays the same stream independently.
  const streamSource =
    mediaStream ??
    (audioElement?.srcObject instanceof MediaStream ? audioElement.srcObject : null)

  if (streamSource) {
    audioCtx.createMediaStreamSource(streamSource).connect(analyser)
  } else if (audioElement) {
    const source = audioCtx.createMediaElementSource(audioElement)
    source.connect(analyser)
    analyser.connect(audioCtx.destination)
  } else {
    throw new Error('attachAudioVisualizer requires audioElement or mediaStream')
  }

  const ensureRunning = () => {
    if (audioCtx.state === 'suspended') {
      void audioCtx.resume()
    }
  }

  if (audioElement) {
    audioElement.addEventListener('playing', ensureRunning)
  }
  for (const track of streamSource?.getAudioTracks() ?? []) {
    track.addEventListener('unmute', ensureRunning)
  }

  const timeData = new Uint8Array(analyser.fftSize)
  const freqData = new Uint8Array(analyser.frequencyBinCount)
  let rafId = 0
  let stopped = false

  const draw = () => {
    if (stopped) return

    rafId = requestAnimationFrame(draw)
    ensureRunning()

    const width = canvas.width
    const height = canvas.height
    const waveHeight = Math.floor(height * 0.55)
    const barHeight = height - waveHeight

    ctx.fillStyle = '#0b1018'
    ctx.fillRect(0, 0, width, height)

    analyser.getByteTimeDomainData(timeData)
    ctx.lineWidth = 2
    ctx.strokeStyle = waveColor
    ctx.beginPath()
    const sliceWidth = width / timeData.length
    let x = 0
    const waveMid = waveHeight / 2
    for (let i = 0; i < timeData.length; i += 1) {
      const y = waveMid + ((timeData[i] - 128) / 128) * waveMid
      if (i === 0) {
        ctx.moveTo(x, y)
      } else {
        ctx.lineTo(x, y)
      }
      x += sliceWidth
    }
    ctx.stroke()

    ctx.strokeStyle = '#1e293b'
    ctx.lineWidth = 1
    ctx.beginPath()
    ctx.moveTo(0, waveHeight)
    ctx.lineTo(width, waveHeight)
    ctx.stroke()

    analyser.getByteFrequencyData(freqData)
    const barCount = 48
    const step = Math.floor(freqData.length / barCount)
    const barWidth = width / barCount - 2

    for (let i = 0; i < barCount; i += 1) {
      let sum = 0
      for (let j = 0; j < step; j += 1) {
        sum += freqData[i * step + j]
      }
      const avg = sum / step / 255
      const h = avg * barHeight
      const bx = i * (barWidth + 2)
      const by = waveHeight + (barHeight - h)
      ctx.fillStyle = barColor
      ctx.fillRect(bx, by, barWidth, h)
    }
  }

  const resume = ensureRunning

  ensureRunning()
  draw()

  return {
    stop() {
      stopped = true
      cancelAnimationFrame(rafId)
      if (audioElement) {
        audioElement.removeEventListener('playing', ensureRunning)
      }
      void audioCtx.close()
    },
    resume,
  }
}
