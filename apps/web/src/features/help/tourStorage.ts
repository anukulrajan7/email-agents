const KEY = 'tour:v1:done';

export const isTourDone = () => localStorage.getItem(KEY) === '1';
export const markTourDone = () => localStorage.setItem(KEY, '1');
