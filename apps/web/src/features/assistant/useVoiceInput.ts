import { useCallback, useEffect, useRef, useState } from 'react';
import { canRecord, createRecognition, type Recognition } from './speech';

export const MAX_RECORDING_MS = 58_000;
const MIME_CANDIDATES = ['audio/webm;codecs=opus', 'audio/webm', 'audio/mp4'];

export type VoiceState = 'idle' | 'recording';

function pickMime(): string | undefined {
  if (typeof MediaRecorder.isTypeSupported !== 'function') return undefined;
  return MIME_CANDIDATES.find((m) => MediaRecorder.isTypeSupported(m));
}

/** Push-to-talk recording. `onRecorded` gets the audio; `partial` is a live caption preview when the browser has one. */
export function useVoiceInput(onRecorded: (blob: Blob, durationMs: number) => void, onProblem: (message: string) => void) {
  const [state, setState] = useState<VoiceState>('idle');
  const [partial, setPartial] = useState('');
  const recorder = useRef<MediaRecorder | undefined>(undefined);
  const recognition = useRef<Recognition | undefined>(undefined);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const callbacks = useRef({ onRecorded, onProblem });
  callbacks.current = { onRecorded, onProblem };
  const supported = canRecord();

  const stop = useCallback(() => {
    clearTimeout(timer.current);
    if (recorder.current && recorder.current.state !== 'inactive') recorder.current.stop();
    recognition.current?.abort();
    recognition.current = undefined;
  }, []);

  const start = useCallback(async () => {
    if (recorder.current) return;
    let stream: MediaStream;
    try {
      stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    } catch {
      callbacks.current.onProblem('The microphone is blocked. Type your request instead.');
      return;
    }
    const mimeType = pickMime();
    const rec = new MediaRecorder(stream, mimeType ? { mimeType } : undefined);
    const chunks: Blob[] = [];
    const startedAt = Date.now();
    rec.ondataavailable = (e) => {
      if (e.data.size > 0) chunks.push(e.data);
    };
    rec.onstop = () => {
      stream.getTracks().forEach((t) => t.stop());
      recorder.current = undefined;
      setState('idle');
      setPartial('');
      const blob = new Blob(chunks, { type: rec.mimeType || mimeType || 'audio/webm' });
      if (blob.size > 0) callbacks.current.onRecorded(blob, Date.now() - startedAt);
    };
    recorder.current = rec;
    rec.start();
    setState('recording');
    timer.current = setTimeout(stop, MAX_RECORDING_MS);

    const captions = createRecognition();
    if (captions) {
      captions.onresult = (e) =>
        setPartial(
          Array.from(e.results)
            .map((r) => r[0].transcript)
            .join(' ')
            .trim(),
        );
      captions.onerror = () => undefined;
      recognition.current = captions;
      try {
        captions.start();
      } catch {
        recognition.current = undefined;
      }
    }
  }, [stop]);

  useEffect(
    () => () => {
      clearTimeout(timer.current);
      const rec = recorder.current;
      if (rec && rec.state !== 'inactive') {
        rec.onstop = null;
        rec.stop();
        rec.stream.getTracks().forEach((t) => t.stop());
      }
      recognition.current?.abort();
    },
    [],
  );

  return { state, partial, supported, start, stop };
}
