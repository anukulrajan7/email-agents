/** Feature checks for screens that land with later work packages. Off by default; turn on with VITE_FEATURE_<NAME>=1 or localStorage `feature:<name>` = "1". */
export type FeatureName = 'agent-builder';

const ENV_KEYS: Record<FeatureName, string | undefined> = {
  'agent-builder': import.meta.env.VITE_FEATURE_AGENT_BUILDER as string | undefined,
};

export function featureEnabled(name: FeatureName): boolean {
  return ENV_KEYS[name] === '1' || localStorage.getItem(`feature:${name}`) === '1';
}
