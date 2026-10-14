export type GuideStep = { text: string; anchor?: string };

export type Guide = {
  id: string;
  title: string;
  topic: string;
  what: string;
  page: string;
  anchor: string;
  avatar: string;
  steps: GuideStep[];
  safe?: string;
  startHere?: true;
  nudge?: true;
  adminOnly?: true;
  pack?: 'staffing';
};

export const GUIDES: Guide[] = [
  {
    id: 'inbox',
    title: 'Inbox and reader',
    topic: 'your inbox',
    what: 'See every email in one list and read what each one is asking.',
    page: 'inbox',
    anchor: 'inbox-list',
    avatar: 'owl',
    startHere: true,
    nudge: true,
    steps: [
      { text: 'Pick an email from the list on the left.', anchor: 'inbox-list' },
      { text: 'Read it here. Under the email you can see its label and a short summary of what it wants.', anchor: 'inbox-reader' },
    ],
    safe: 'Reading an email never changes it and never tells the sender anything.',
  },
  {
    id: 'replies',
    title: 'Replies, drafts and undo send',
    topic: 'replies',
    what: 'Get a draft reply written for you, change the words, and send it with a short chance to take it back.',
    page: 'inbox',
    anchor: 'draft-panel',
    avatar: 'rocket',
    startHere: true,
    steps: [
      { text: 'Open an email that needs an answer.', anchor: 'inbox-list' },
      { text: 'Press Generate draft. A reply appears in a few seconds.', anchor: 'draft-panel' },
      { text: 'Change any words you like. Your edits teach the agent how you write.', anchor: 'draft-panel' },
      { text: 'Press Send. For a short moment an Undo button stays on screen. Press it to pull the email back.', anchor: 'draft-panel' },
    ],
    safe: 'Nothing leaves until you press Send, and Undo is there right after.',
  },
  {
    id: 'meera',
    title: 'Ask Meera',
    topic: 'Ask Meera',
    what: 'Ask questions about your mail, or how things work here, and get a plain answer.',
    page: 'inbox',
    anchor: 'ask-meera',
    avatar: 'blossom',
    startHere: true,
    steps: [
      { text: 'Press Ask Meera in the corner, or Ctrl or Cmd plus K.', anchor: 'ask-meera' },
      { text: 'Open an email first and ask, "What does this want from me?" Meera can see the email you have open.', anchor: 'ask-meera' },
      { text: 'Attach a sheet or a file and ask about it. Meera reads it and answers from what is inside.', anchor: 'ask-meera' },
    ],
    safe: 'Meera answers and suggests. She does not send or change anything without your OK.',
  },
  {
    id: 'labels',
    title: 'Labels',
    topic: 'labels',
    what: 'Labels are the categories your mail is sorted into.',
    page: 'labels',
    anchor: 'labels-list',
    avatar: 'bee',
    nudge: true,
    steps: [
      { text: 'Read down the list. Each label has a short description.', anchor: 'labels-list' },
      { text: 'Press Edit on a label and rewrite its description as if you were briefing a new teammate. The better the brief, the better the sorting.', anchor: 'labels-list' },
      { text: 'Open the Health tab to see which labels are being mixed up.' },
    ],
    safe: 'Archiving a label keeps your old mail. New mail just stops going into it.',
  },
  {
    id: 'team',
    title: 'Your team of agents',
    topic: 'your team',
    what: 'Each agent looks after one kind of email. Meera leads the team.',
    page: 'agent-team',
    anchor: 'team-header',
    avatar: 'panda',
    nudge: true,
    steps: [
      { text: 'Every card below is one helper. Read what it does and how much it has done today.', anchor: 'team-header' },
      { text: 'Use the switch on a card to pause it or turn it back on.' },
      { text: 'Press Tune and history to rename an agent, pick an avatar or go back to an earlier setup.' },
    ],
    safe: 'Pausing an agent deletes nothing. You can turn it back on any time.',
  },
  {
    id: 'autonomy',
    title: 'Freedom levels and the 10-minute hold',
    topic: 'freedom levels',
    what: 'You decide how much an agent may do by itself.',
    page: 'agent-team',
    anchor: 'autonomy-chip',
    avatar: 'robot',
    steps: [
      { text: 'Find the grey tag on a card. It says how much freedom that agent has right now.', anchor: 'autonomy-chip' },
      { text: 'Press Tune and history and choose a limit: watching, asks first, routine replies or trusted.' },
      { text: 'Agents that send routine replies wait 10 minutes first. Press Stop in that time to cancel the send.' },
    ],
    safe: 'Agents start careful, and you can lower the limit at any time.',
  },
  {
    id: 'approvals',
    title: 'Waiting for you',
    topic: 'approvals',
    what: 'When an agent wants to send something it is not allowed to send alone, it asks you here first.',
    page: 'agents',
    anchor: 'approvals',
    avatar: 'turtle',
    nudge: true,
    steps: [
      { text: 'Look under Waiting for you. Each item is a draft that needs your OK.', anchor: 'approvals' },
      { text: 'Read the draft and change it if you want.', anchor: 'approvals' },
      { text: 'Press approve to send it, or decline to drop it.', anchor: 'approvals' },
    ],
    safe: 'An agent cannot send something important without your approval.',
  },
  {
    id: 'memory',
    title: 'Memory and learning',
    topic: 'memory',
    what: 'See what your agents have learned about senders, your style and your team.',
    page: 'memory',
    anchor: 'memory-learned',
    avatar: 'koala',
    nudge: true,
    steps: [
      { text: 'Read what was learned. Items marked as learned automatically came from your mail.', anchor: 'memory-learned' },
      { text: 'Pin what matters, confirm what looks right and delete what is wrong.', anchor: 'memory-learned' },
    ],
    safe: 'You can delete anything an agent has learned, and it stops using it.',
  },
  {
    id: 'automations',
    title: 'Automations',
    topic: 'automations',
    what: 'Rules that run on each new email, such as drafting a reply or reminding you when a thread goes quiet.',
    page: 'automations',
    anchor: 'automations-header',
    avatar: 'hedgehog',
    nudge: true,
    steps: [
      { text: 'Read each rule like a sentence: when an email looks like this, do these steps.', anchor: 'automations-header' },
      { text: 'Admins can press New automation to write their own. A preview reads it back to you before you save.', anchor: 'automations-header' },
    ],
    safe: 'You see a plain-English preview of the rule before it goes live.',
  },
  {
    id: 'alerts',
    title: 'Alerts',
    topic: 'alerts',
    what: 'Get told when an important email is still unanswered.',
    page: 'alerts',
    anchor: 'alerts-tabs',
    avatar: 'cat',
    nudge: true,
    steps: [
      { text: 'Switch between the tabs to see open and finished alerts.', anchor: 'alerts-tabs' },
      { text: 'Open an alert to jump to the email, then reply or dismiss it.', anchor: 'alerts-tabs' },
    ],
  },
  {
    id: 'packs',
    title: 'Packs',
    topic: 'packs',
    what: 'A pack is a bundle for one kind of work: labels, ready-made agents and reply style.',
    page: 'packs',
    anchor: 'packs-card',
    avatar: 'otter',
    adminOnly: true,
    nudge: true,
    steps: [
      { text: 'Tick the kinds of work your team does.', anchor: 'packs-card' },
      { text: 'Save. The right agents and labels are added for you.', anchor: 'packs-card' },
    ],
    safe: 'Turning a pack off pauses its agents and keeps all your data.',
  },
  {
    id: 'matching',
    title: 'Matching',
    topic: 'matching',
    what: 'Line up client requirements with referred candidates and see the best fits first.',
    page: 'matching',
    anchor: 'matching-header',
    avatar: 'fox',
    pack: 'staffing',
    nudge: true,
    steps: [
      { text: 'Pick a requirement from the list.', anchor: 'matching-header' },
      { text: 'Read the candidates ranked for it. Each one shows why it fits.', anchor: 'matching-header' },
      { text: 'Open the source email to check the details before you act.', anchor: 'matching-header' },
    ],
    safe: 'Matching only suggests. Nobody is contacted unless you choose to.',
  },
  {
    id: 'hr-workflows',
    title: 'Timesheets, leave and referrals',
    topic: 'HR workflows',
    what: 'Ready-made follow-ups for HR emails, such as opening a review task for a timesheet.',
    page: 'workflows',
    anchor: 'workflows-grid',
    avatar: 'penguin',
    pack: 'staffing',
    nudge: true,
    steps: [
      { text: 'Each card is one follow-up and the label it runs on.', anchor: 'workflows-grid' },
      { text: 'Admins can turn a card on with its switch. If it says Needs label, one button adds the label for you.', anchor: 'workflows-grid' },
    ],
    safe: 'You can switch any workflow off again without losing past runs.',
  },
  {
    id: 'people',
    title: 'People',
    topic: 'your people',
    what: 'The list of everyone on your team, with their emails, manager and targets.',
    page: 'people',
    anchor: 'people-header',
    avatar: 'owl',
    pack: 'staffing',
    nudge: true,
    steps: [
      { text: 'Read the list. Each person has their email addresses, a manager and a joining date.', anchor: 'people-header' },
      { text: 'Owners and admins can press Add person, or Import CSV to add many at once.', anchor: 'people-header' },
    ],
    safe: 'Only owners and admins can add or change people.',
  },
  {
    id: 'performance',
    title: 'Performance',
    topic: 'performance',
    what: 'See how many emails each person handled today, this week and since joining, against their target.',
    page: 'performance',
    anchor: 'performance-header',
    avatar: 'fox',
    pack: 'staffing',
    nudge: true,
    steps: [
      { text: 'Look at Your numbers to see today and this week against your target.', anchor: 'performance-numbers' },
      { text: 'Below that, see how the rest of the team is doing, if you are allowed to see it.', anchor: 'performance-header' },
    ],
    safe: 'Every number is counted from your mail. Nothing is guessed.',
  },
  {
    id: 'performance-widget',
    title: 'Your numbers widget',
    topic: 'the numbers widget',
    what: 'A small box in the corner that shows your numbers for today and this week.',
    page: 'performance',
    anchor: 'performance-widget',
    avatar: 'rocket',
    pack: 'staffing',
    steps: [
      { text: 'Look at the bottom right of the screen. The box shows today and this week against your target.', anchor: 'performance-widget' },
      { text: 'Press the box to open the full Performance page.', anchor: 'performance-widget' },
    ],
    safe: 'Only people in the team list see the box.',
  },
  {
    id: 'timesheets',
    title: 'Timesheets',
    topic: 'timesheets',
    what: 'Track who has sent their monthly timesheet and who is late.',
    page: 'timesheets',
    anchor: 'timesheets-header',
    avatar: 'bee',
    pack: 'staffing',
    nudge: true,
    steps: [
      { text: 'Read the list to see who has sent a timesheet this month.', anchor: 'timesheets-header' },
      { text: 'Owners can change how many days after month end a timesheet counts as late.', anchor: 'timesheets-header' },
    ],
    safe: 'Only owners can change the late rule, and past timesheets are kept.',
  },
  {
    id: 'portal',
    title: 'Portal reconciliation',
    topic: 'portal reconciliation',
    what: 'Compare hours and submissions from the client portal with what your mail shows.',
    page: 'portal',
    anchor: 'portal-header',
    avatar: 'turtle',
    pack: 'staffing',
    nudge: true,
    steps: [
      { text: 'Import the hours and submissions you exported from the client portal.', anchor: 'portal-header' },
      { text: 'Press Compare. Every difference becomes an alert you can open.', anchor: 'portal-header' },
    ],
    safe: 'Comparing only reads. It never changes the portal or your mail.',
  },
  {
    id: 'escalations',
    title: 'Escalations',
    topic: 'escalations',
    what: 'Decide who is told when an alert is ignored, and how long to wait before each step.',
    page: 'escalations',
    anchor: 'escalations-header',
    avatar: 'robot',
    pack: 'staffing',
    nudge: true,
    steps: [
      { text: 'Read who is told, in order: the person, then their manager, then HR or the admins.', anchor: 'escalations-chains' },
      { text: 'Owners can change the delays between each step.', anchor: 'escalations-header' },
    ],
    safe: 'Only owners and admins can change who is told and when.',
  },
  {
    id: 'usage',
    title: 'AI usage',
    topic: 'AI usage',
    what: 'See what the assistant costs today and where the money goes.',
    page: 'usage',
    anchor: 'usage-today',
    avatar: 'dolphin',
    adminOnly: true,
    nudge: true,
    steps: [
      { text: 'Check today against your daily budget.', anchor: 'usage-today' },
      { text: 'Scroll down to see which tasks cost the most.' },
    ],
  },
];

export type GuideAudience = { isAdmin: boolean; hasStaffing: boolean };

export const visibleGuides = ({ isAdmin, hasStaffing }: GuideAudience): Guide[] =>
  GUIDES.filter((g) => (!g.adminOnly || isAdmin) && (g.pack !== 'staffing' || hasStaffing));

export const startHereGuides = (audience: GuideAudience): Guide[] => visibleGuides(audience).filter((g) => g.startHere);

export const nudgeGuideFor = (page: string, audience: GuideAudience): Guide | undefined =>
  visibleGuides(audience).find((g) => g.nudge && g.page === page);

export const pageOfPath = (pathname: string): string => pathname.split('/')[3] ?? '';

export const stepAnchor = (guide: Guide, step: GuideStep): string => step.anchor ?? guide.anchor;
