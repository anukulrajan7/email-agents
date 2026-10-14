type RecognitionResult = { isFinal: boolean; 0: { transcript: string } };
type RecognitionEvent = { results: ArrayLike<RecognitionResult> };

export type Recognition = {
  continuous: boolean;
  interimResults: boolean;
  lang: string;
  onresult: ((e: RecognitionEvent) => void) | null;
  onerror: (() => void) | null;
  start: () => void;
  abort: () => void;
};

type RecognitionCtor = new () => Recognition;

/** Browser live captions, when available. Only a preview: the server transcript is what gets sent. */
export function createRecognition(): Recognition | undefined {
  const w = window as unknown as { SpeechRecognition?: RecognitionCtor; webkitSpeechRecognition?: RecognitionCtor };
  const Ctor = w.SpeechRecognition ?? w.webkitSpeechRecognition;
  if (!Ctor) return undefined;
  const rec = new Ctor();
  rec.continuous = true;
  rec.interimResults = true;
  rec.lang = navigator.language || 'en-US';
  return rec;
}

export const canSpeak = () => typeof window !== 'undefined' && 'speechSynthesis' in window && typeof SpeechSynthesisUtterance !== 'undefined';

/** Plain words only: no markdown, links or citation markers read aloud. */
export function speakableText(text: string): string {
  return text
    .replace(/\[(?:\d+|[a-z]_[\w-]+)\]/gi, '')
    .replace(/\[([^\]]+)\]\([^)]+\)/g, '$1')
    .replace(/[*_`#>]+/g, '')
    .replace(/\s+/g, ' ')
    .trim();
}

/** Short pieces: Chrome stops a long utterance after about 15 seconds. */
export function speechChunks(text: string, max = 180): string[] {
  const sentences = speakableText(text).match(/[^.!?]+[.!?]*\s*/g) ?? [];
  const chunks: string[] = [];
  let current = '';
  for (const s of sentences) {
    if ((current + s).length > max && current) {
      chunks.push(current.trim());
      current = '';
    }
    current += s;
  }
  if (current.trim()) chunks.push(current.trim());
  return chunks;
}

function voiceFor(lang: string): SpeechSynthesisVoice | undefined {
  const voices = window.speechSynthesis.getVoices();
  const base = lang.split('-')[0] ?? lang;
  return voices.find((v) => v.lang === lang) ?? voices.find((v) => v.lang.startsWith(base)) ?? voices.find((v) => v.default);
}

export function speak(text: string) {
  if (!canSpeak()) return;
  const synth = window.speechSynthesis;
  synth.cancel();
  const lang = navigator.language || 'en-US';
  // Chrome drops an utterance queued in the same tick as cancel(), and can be left paused.
  window.setTimeout(() => {
    synth.resume();
    for (const chunk of speechChunks(text)) {
      const u = new SpeechSynthesisUtterance(chunk);
      u.lang = lang;
      const voice = voiceFor(lang);
      if (voice) u.voice = voice;
      synth.speak(u);
    }
  }, 60);
}

/** Call from the click that turns read-aloud on: Safari only speaks after a gesture unlocked audio. */
export function unlockSpeech() {
  if (!canSpeak()) return;
  window.speechSynthesis.getVoices();
  const u = new SpeechSynthesisUtterance(' ');
  u.volume = 0;
  window.speechSynthesis.speak(u);
}

export function stopSpeaking() {
  if (canSpeak()) window.speechSynthesis.cancel();
}

export const canRecord = () =>
  typeof navigator !== 'undefined' && typeof navigator.mediaDevices?.getUserMedia === 'function' && typeof MediaRecorder !== 'undefined';
