import { createContext, useCallback, useContext, useMemo, useState, type ReactNode } from 'react';
import { visibleGuides, type Guide } from './guides';
import { readDismissed, writeDismissed } from './guideStorage';

type GuideContextValue = {
  ws: string;
  guides: Guide[];
  dismissed: ReadonlySet<string>;
  dismiss: (id: string) => void;
  restoreAll: () => void;
};

const GuideContext = createContext<GuideContextValue | undefined>(undefined);

export function useGuide(): GuideContextValue {
  const value = useContext(GuideContext);
  if (!value) throw new Error('useGuide must be used inside GuideProvider');
  return value;
}

export function GuideProvider({
  ws,
  user,
  isAdmin,
  hasStaffing,
  children,
}: {
  ws: string;
  user: string;
  isAdmin: boolean;
  hasStaffing: boolean;
  children: ReactNode;
}) {
  const [dismissed, setDismissed] = useState(() => readDismissed(user, ws));
  const guides = useMemo(() => visibleGuides({ isAdmin, hasStaffing }), [isAdmin, hasStaffing]);

  const dismiss = useCallback(
    (id: string) =>
      setDismissed((prev) => {
        const next = new Set(prev).add(id);
        writeDismissed(user, ws, next);
        return next;
      }),
    [user, ws],
  );

  const restoreAll = useCallback(() => {
    const none = new Set<string>();
    writeDismissed(user, ws, none);
    setDismissed(none);
  }, [user, ws]);

  const value = useMemo(() => ({ ws, guides, dismissed, dismiss, restoreAll }), [ws, guides, dismissed, dismiss, restoreAll]);
  return <GuideContext.Provider value={value}>{children}</GuideContext.Provider>;
}
