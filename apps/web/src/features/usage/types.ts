export interface DayUsage {
  date: string;
  usd: number;
  calls: number;
}

export interface TaskUsage {
  task: string;
  usd: number;
  calls: number;
}

export interface AiUsage {
  days: number;
  total_usd: number;
  today_usd: number;
  budget_usd_per_day: number;
  by_day: DayUsage[];
  by_task: TaskUsage[];
  per_email_usd: number;
}
