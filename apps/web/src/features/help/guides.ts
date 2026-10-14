export type Guide = {
  id: string;
  title: string;
  what: string;
  page: string;
  avatar: string;
  steps: string[];
  safe?: string;
  adminOnly?: true;
  pack?: 'staffing';
};

export const GUIDES: Guide[] = [
  {
    id: 'inbox',
    title: 'Inbox and reader',
    what: 'See every email in one list and read what each one is asking.',
    page: 'inbox',
    avatar: 'owl',
    steps: [
      'Pick an email from the list on the left.',
      'Read it here. Under the email you can see its label and a short summary of what it wants.',
    ],
    safe: 'Reading an email never changes it and never tells the sender anything.',
  },
  {
    id: 'replies',
    title: 'Replies, drafts and undo send',
    what: 'Get a draft reply written for you, change the words, and send it with a short chance to take it back.',
    page: 'inbox',
    avatar: 'rocket',
    steps: [
      'Open an email that needs an answer.',
      'Press Generate draft. A reply appears in a few seconds.',
      'Change any words you like. Your edits teach the agent how you write.',
      'Press Send. For a short moment an Undo button stays on screen. Press it to pull the email back.',
    ],
    safe: 'Nothing leaves until you press Send, and Undo is there right after.',
  },
  {
    id: 'meera',
    title: 'Ask Meera',
    what: 'Ask questions about your mail, or how things work here, and get a plain answer.',
    page: 'inbox',
    avatar: 'blossom',
    steps: [
      'Press Ask Meera in the corner, or Ctrl or Cmd plus K.',
      'Open an email first and ask, "What does this want from me?" Meera can see the email you have open.',
      'Attach a sheet or a file and ask about it. Meera reads it and answers from what is inside.',
    ],
    safe: 'Meera answers and suggests. She does not send or change anything without your OK.',
  },
  {
    id: 'labels',
    title: 'Labels',
    what: 'Labels are the categories your mail is sorted into.',
    page: 'labels',
    avatar: 'bee',
    steps: [
      'Read down the list. Each label has a short description.',
      'Press Edit on a label and rewrite its description as if you were briefing a new teammate. The better the brief, the better the sorting.',
      'Open the Health tab to see which labels are being mixed up.',
    ],
    safe: 'Archiving a label keeps your old mail. New mail just stops going into it.',
  },
  {
    id: 'team',
    title: 'Your team of agents',
    what: 'Each agent looks after one kind of email. Aarav leads the team, and Meera is your own assistant.',
    page: 'agent-team',
    avatar: 'panda',
    steps: [
      'Every card below is one helper. Read what it does and how much it has done today.',
      'Use the switch on a card to pause it or turn it back on.',
      'Press Tune and history to rename an agent, pick an avatar or go back to an earlier setup.',
    ],
    safe: 'Pausing an agent deletes nothing. You can turn it back on any time.',
  },
  {
    id: 'autonomy',
    title: 'Freedom levels and the 10-minute hold',
    what: 'You decide how much an agent may do by itself.',
    page: 'agent-team',
    avatar: 'robot',
    steps: [
      'Find the grey tag on a card. It says how much freedom that agent has right now.',
      'Press Tune and history and choose a limit: watching, asks first, routine replies or trusted.',
      'Agents that send routine replies wait 10 minutes first. Press Stop in that time to cancel the send.',
    ],
    safe: 'Agents start careful, and you can lower the limit at any time.',
  },
  {
    id: 'approvals',
    title: 'Waiting for you',
    what: 'When an agent wants to send something it is not allowed to send alone, it asks you here first.',
    page: 'agents',
    avatar: 'turtle',
    steps: [
      'Look under Waiting for you. Each item is a draft that needs your OK.',
      'Read the draft and change it if you want.',
      'Press approve to send it, or decline to drop it.',
    ],
    safe: 'An agent cannot send something important without your approval.',
  },
  {
    id: 'memory',
    title: 'Memory and learning',
    what: 'See what your agents have learned about senders, your style and your team.',
    page: 'memory',
    avatar: 'koala',
    steps: [
      'Read what was learned. Items marked as learned automatically came from your mail.',
      'Pin what matters, confirm what looks right and delete what is wrong.',
    ],
    safe: 'You can delete anything an agent has learned, and it stops using it.',
  },
  {
    id: 'automations',
    title: 'Automations',
    what: 'Rules that run on each new email, such as drafting a reply or reminding you when a thread goes quiet.',
    page: 'automations',
    avatar: 'hedgehog',
    steps: [
      'Read each rule like a sentence: when an email looks like this, do these steps.',
      'Admins can press New automation to write their own. A preview reads it back to you before you save.',
    ],
    safe: 'You see a plain-English preview of the rule before it goes live.',
  },
  {
    id: 'alerts',
    title: 'Alerts',
    what: 'Get told when an important email is still unanswered.',
    page: 'alerts',
    avatar: 'cat',
    steps: [
      'Switch between the tabs to see open and finished alerts.',
      'Open an alert to jump to the email, then reply or dismiss it.',
    ],
  },
  {
    id: 'packs',
    title: 'Packs',
    what: 'A pack is a bundle for one kind of work: labels, ready-made agents and reply style.',
    page: 'packs',
    avatar: 'otter',
    adminOnly: true,
    steps: [
      'Tick the kinds of work your team does.',
      'Save. The right agents and labels are added for you.',
    ],
    safe: 'Turning a pack off pauses its agents and keeps all your data.',
  },
  {
    id: 'matching',
    title: 'Matching',
    what: 'Line up client requirements with referred candidates and see the best fits first.',
    page: 'matching',
    avatar: 'fox',
    pack: 'staffing',
    steps: [
      'Pick a requirement from the list.',
      'Read the candidates ranked for it. Each one shows why it fits.',
      'Open the source email to check the details before you act.',
    ],
    safe: 'Matching only suggests. Nobody is contacted unless you choose to.',
  },
  {
    id: 'hr-workflows',
    title: 'Timesheets, leave and referrals',
    what: 'Ready-made follow-ups for HR emails, such as opening a review task for a timesheet.',
    page: 'workflows',
    avatar: 'penguin',
    pack: 'staffing',
    steps: [
      'Each card is one follow-up and the label it runs on.',
      'Admins can turn a card on with its switch. If it says Needs label, one button adds the label for you.',
    ],
    safe: 'You can switch any workflow off again without losing past runs.',
  },
  {
    id: 'people',
    title: 'People',
    what: 'The list of everyone on your team, with their emails, manager and targets.',
    page: 'people',
    avatar: 'owl',
    pack: 'staffing',
    steps: [
      'Read the list. Each person has their email addresses, a manager and a joining date.',
      'Owners and admins can press Add person, or Import CSV to add many at once.',
    ],
    safe: 'Only owners and admins can add or change people.',
  },
  {
    id: 'performance',
    title: 'Performance',
    what: 'See how many emails each person handled today, this week and since joining, against their target.',
    page: 'performance',
    avatar: 'fox',
    pack: 'staffing',
    steps: [
      'Look at Your numbers to see today and this week against your target.',
      'Below that, see how the rest of the team is doing, if you are allowed to see it.',
    ],
    safe: 'Every number is counted from your mail. Nothing is guessed.',
  },
  {
    id: 'performance-widget',
    title: 'Your numbers widget',
    what: 'A small box in the corner that shows your numbers for today and this week.',
    page: 'performance',
    avatar: 'rocket',
    pack: 'staffing',
    steps: [
      'Look at the bottom right of the screen. The box shows today and this week against your target.',
      'Press the box to open the full Performance page.',
    ],
    safe: 'Only people in the team list see the box.',
  },
  {
    id: 'timesheets',
    title: 'Timesheets',
    what: 'Track who has sent their monthly timesheet and who is late.',
    page: 'timesheets',
    avatar: 'bee',
    pack: 'staffing',
    steps: [
      'Read the list to see who has sent a timesheet this month.',
      'Owners can change how many days after month end a timesheet counts as late.',
    ],
    safe: 'Only owners can change the late rule, and past timesheets are kept.',
  },
  {
    id: 'portal',
    title: 'Portal reconciliation',
    what: 'Compare hours and submissions from the client portal with what your mail shows.',
    page: 'portal',
    avatar: 'turtle',
    pack: 'staffing',
    steps: [
      'Import the hours and submissions you exported from the client portal.',
      'Press Compare now. Every difference becomes an alert you can open.',
    ],
    safe: 'Comparing only reads. It never changes the portal or your mail.',
  },
  {
    id: 'escalations',
    title: 'Escalations',
    what: 'Decide who is told when an alert is ignored, and how long to wait before each step.',
    page: 'escalations',
    avatar: 'robot',
    pack: 'staffing',
    steps: [
      'Read who is told, in order: the person, then their manager, then HR or the admins.',
      'Owners can change the delays between each step.',
    ],
    safe: 'Only owners and admins can change who is told and when.',
  },
  {
    id: 'usage',
    title: 'AI usage',
    what: 'See what the assistant costs today and where the money goes.',
    page: 'usage',
    avatar: 'dolphin',
    adminOnly: true,
    steps: [
      'Check today against your daily budget.',
      'Scroll down to see which tasks cost the most.',
    ],
  },
];

export type GuideAudience = { isAdmin: boolean; hasStaffing: boolean };

export const visibleGuides = ({ isAdmin, hasStaffing }: GuideAudience): Guide[] =>
  GUIDES.filter((g) => (!g.adminOnly || isAdmin) && (g.pack !== 'staffing' || hasStaffing));

export const pageOfPath = (pathname: string): string => pathname.split('/')[3] ?? '';

/** The one card a page shows at its top; pages with several guides use the first. */
export const guideForPage = (page: string, audience: GuideAudience): Guide | undefined => visibleGuides(audience).find((g) => g.page === page);
