export type HelpTopic = 'agents' | 'autonomy' | 'packs' | 'memory' | 'approvals' | 'hrms' | 'welcome';

export type HelpEntry = { title: string; body: string[]; points?: { term: string; text: string }[] };

export const HELP: Record<HelpTopic, HelpEntry> = {
  agents: {
    title: 'Your agents',
    body: [
      'An agent is a helper that handles one kind of email for you: it reads, sorts, drafts replies and follows up.',
      'Meera leads the team and picks the right agent for each message. You also have a personal agent that works only on your own mailbox. Nobody else can see it, admins included.',
    ],
    points: [
      { term: 'Pause', text: 'Stops an agent. Nothing is deleted, and you can turn it back on any time.' },
      { term: 'Tune', text: 'Change its name, avatar and how much freedom it has.' },
      { term: 'Versions', text: 'Every change is saved, so you can roll back to an earlier setup.' },
    ],
  },
  autonomy: {
    title: 'How much freedom an agent has',
    body: ['Agents start careful and earn trust. You can always lower the limit; you never have to raise it.'],
    points: [
      { term: 'Watching', text: 'Learns from your mail without touching it.' },
      { term: 'Asks first', text: 'Writes drafts. A person approves every send.' },
      { term: 'Routine replies', text: 'Sends simple, safe replies itself after a 10-minute hold. Press Stop in that window to cancel.' },
      { term: 'Trusted', text: 'Also replies to people already in the conversation, with the same hold.' },
    ],
  },
  packs: {
    title: 'What is a pack?',
    body: [
      'A pack is a bundle for one kind of work: labels, ready-made agents and reply style.',
      'Turning a pack on adds its agents. Turning it off pauses them and keeps your data, so you can switch it back on later.',
    ],
  },
  memory: {
    title: 'What is memory?',
    body: [
      'Memory is what your agents have learned: facts about senders, how you like to reply, and what matters to your team.',
      'Pin what is important, confirm what looks right and delete what is wrong. Agents use memory when they sort mail and write drafts.',
    ],
  },
  approvals: {
    title: 'What are approvals?',
    body: [
      'When an agent wants to send something and is not allowed to do it alone, it asks you first.',
      'Read the draft, edit it if you like, then approve or decline. Your edits teach the agent how you write.',
    ],
  },
  hrms: {
    title: 'HRMS desks',
    body: ['The HR pack gives you one desk per kind of request, so each email reaches the right specialist.'],
    points: [
      { term: 'Leave', text: 'Leave requests and balances.' },
      { term: 'Timesheets', text: 'Submitted hours and reminders.' },
      { term: 'Onboarding', text: 'New joiner paperwork and first-week questions.' },
      { term: 'Candidates & Requirements', text: 'Client requirements and referred candidates.' },
      { term: 'Payroll queries', text: 'Salary and payslip questions.' },
    ],
  },
  welcome: {
    title: 'Getting set up',
    body: [
      'Tell us what you want help with and we switch on the right team of agents.',
      'Connect a mailbox so they can see your mail. They start by watching and drafting, and ask you before sending anything.',
    ],
  },
};
