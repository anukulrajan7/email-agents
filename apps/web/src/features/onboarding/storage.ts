const dismissedKey = (ws: string) => `getting-started:${ws}:dismissed`;
const labelsKey = (ws: string) => `getting-started:${ws}:labels-visited`;
const repliesKey = (ws: string) => `getting-started:${ws}:reply-setup-done`;

export const isDismissed = (ws: string) => localStorage.getItem(dismissedKey(ws)) === '1';
export const dismiss = (ws: string) => localStorage.setItem(dismissedKey(ws), '1');
export const hasVisitedLabels = (ws: string) => localStorage.getItem(labelsKey(ws)) === '1';
export const markLabelsVisited = (ws: string) => localStorage.setItem(labelsKey(ws), '1');
export const isReplySetupDone = (ws: string) => localStorage.getItem(repliesKey(ws)) === '1';
export const markReplySetupDone = (ws: string) => localStorage.setItem(repliesKey(ws), '1');
