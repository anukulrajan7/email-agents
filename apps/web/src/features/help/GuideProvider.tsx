import { createContext, useCallback, useContext, useMemo, useRef, useState, type ReactNode } from 'react';
import { useLocation, useNavigate } from 'react-router';
import { Spotlight } from './Spotlight';
import { GUIDES, pageOfPath, startHereGuides, visibleGuides, type Guide } from './guides';
import { readTried, writeTried } from './guideStorage';

type Active = { guideId: string; step: number; queue: string[] };

type GuideContextValue = {
  ws: string;
  user: string;
  guides: Guide[];
  startHere: Guide[];
  tried: ReadonlySet<string>;
  setTried: (id: string, on: boolean) => void;
  start: (id: string, queue?: string[]) => void;
  running: boolean;
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
  const navigate = useNavigate();
  const location = useLocation();
  const [tried, setTriedState] = useState(() => readTried(user, ws));
  const [active, setActive] = useState<Active>();
  const opener = useRef<HTMLElement | null>(null);

  const guides = useMemo(() => visibleGuides({ isAdmin, hasStaffing }), [isAdmin, hasStaffing]);
  const startHere = useMemo(() => startHereGuides({ isAdmin, hasStaffing }), [isAdmin, hasStaffing]);

  const setTried = useCallback(
    (id: string, on: boolean) =>
      setTriedState((prev) => {
        const next = new Set(prev);
        if (on) next.add(id);
        else next.delete(id);
        writeTried(user, ws, next);
        return next;
      }),
    [user, ws],
  );

  const begin = useCallback(
    (id: string, queue: string[]) => {
      const guide = GUIDES.find((g) => g.id === id);
      if (!guide) return;
      if (pageOfPath(location.pathname) !== guide.page) navigate(`/w/${ws}/${guide.page}`);
      setActive({ guideId: id, step: 0, queue });
    },
    [location.pathname, navigate, ws],
  );

  const start = useCallback(
    (id: string, queue: string[] = []) => {
      if (!active) opener.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
      begin(id, queue);
    },
    [active, begin],
  );

  const guide = active ? GUIDES.find((g) => g.id === active.guideId) : undefined;

  const close = (finished: boolean) => {
    if (!active || !guide) return;
    if (finished) {
      setTried(guide.id, true);
      const [next, ...rest] = active.queue;
      if (next) {
        begin(next, rest);
        return;
      }
    }
    setActive(undefined);
    const target = opener.current;
    if (target?.isConnected) target.focus();
  };

  const value = useMemo<GuideContextValue>(
    () => ({ ws, user, guides, startHere, tried, setTried, start, running: active !== undefined }),
    [ws, user, guides, startHere, tried, setTried, start, active],
  );

  return (
    <GuideContext.Provider value={value}>
      {children}
      {active && guide && (
        <Spotlight
          key={guide.id}
          guide={guide}
          step={active.step}
          hasNextGuide={active.queue.length > 0}
          onStep={(step) => setActive({ ...active, step })}
          onClose={close}
        />
      )}
    </GuideContext.Provider>
  );
}
