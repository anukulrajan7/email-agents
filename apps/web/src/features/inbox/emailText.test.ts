import { avatarColor, cleanEmailText, gmailDate, initialOf, parseAddress, splitEmailText } from './emailText';

describe('emailText', () => {
  it('strips conditional comments and unwraps angle-bracket URLs', () => {
    expect(cleanEmailText('<!--[if !mso]><!-->Hi<![endif]--> <https://a.test/x> <!-- note -->bye')).toBe('Hi https://a.test/x bye');
  });

  it('splits history, inline quotes and signature into segments', () => {
    const segments = splitEmailText('Hello\n> quoted\nWorld\n-- \nAda\n\nOn Tue, Oct 14, 2026 at 9:00 AM Bob <b@x.test>\nwrote:\n> older');
    expect(segments.map((s) => s.kind)).toEqual(['text', 'quoted', 'text', 'signature', 'quoted']);
    expect(segments[4]?.text).toContain('wrote:');
  });

  it('does not collapse a message that starts with a quote header', () => {
    const segments = splitEmailText('On Tue, Oct 14, 2026 Bob wrote:\nhi');
    expect(segments).toEqual([{ kind: 'text', text: 'On Tue, Oct 14, 2026 Bob wrote:\nhi' }]);
  });

  it('parses display names and bare addresses', () => {
    expect(parseAddress('"Ada L" <ada@x.test>')).toEqual({ name: 'Ada L', address: 'ada@x.test' });
    expect(parseAddress('ada@x.test')).toEqual({ name: 'ada', address: 'ada@x.test' });
  });

  it('derives deterministic avatar colors and initials', () => {
    expect(avatarColor('ada@x.test')).toBe(avatarColor('ADA@x.test'));
    expect(initialOf('  élan')).toBe('É');
    expect(initialOf('!!')).toBe('?');
  });

  it('formats dates Gmail-style with relative time', () => {
    const now = new Date('2026-10-14T12:00:00Z').getTime();
    expect(gmailDate('2026-10-13T12:00:00Z', now)).toMatch(/^Oct 13, \d{1,2}:\d{2}\s?[AP]M \(yesterday\)$/);
  });
});
