import { describe, expect, it } from 'vitest';
import { speakableText, speechChunks } from './speech';

describe('read aloud text', () => {
  it('drops markdown, links and citation markers', () => {
    expect(speakableText('**Hi** [Invoice](https://x.test) is due [1].')).toBe('Hi Invoice is due .');
  });

  it('splits long answers into short pieces at sentence ends', () => {
    const text = Array.from({ length: 8 }, (_, i) => `Sentence number ${i} is here and has some words.`).join(' ');
    const chunks = speechChunks(text, 120);
    expect(chunks.length).toBeGreaterThan(1);
    expect(chunks.every((c) => c.length <= 120)).toBe(true);
    expect(chunks.join(' ')).toContain('Sentence number 7');
  });
});
