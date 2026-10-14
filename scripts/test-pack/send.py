#!/usr/bin/env python3
"""Sends the live test pack to a connected mailbox through the server's SMTP account.

Run on the VPS (it reads SMTP_* from the app's .env):
  python3 -I send.py --to anukulraj2004@gmail.com [--only ts_pasted,leave_request] [--gap 20] [--dry-run]

Every mail comes FROM the SMTP account (display name = the made-up sender), so any agent reply goes back to that
account, never to a real stranger. Each mail carries `X-PostRoom-Test: <run>:<scenario>`, and its Message-ID is
saved in runs/<run>.json for check.py.
"""

import argparse
import email.utils
import json
import os
import smtplib
import ssl
import sys
import time
from email.message import EmailMessage

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from scenarios import scenarios  # noqa: E402


def env(path):
    out = {}
    for line in open(path, encoding="utf-8"):
        line = line.strip()
        if line and not line.startswith("#") and "=" in line:
            k, v = line.split("=", 1)
            out[k] = v.strip().strip('"').strip("'")
    return out


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--to", required=True)
    p.add_argument("--env", default="/docker/email-agents/.env")
    p.add_argument("--only", default="")
    p.add_argument("--gap", type=float, default=15, help="seconds between mails (keeps order, avoids rate limits)")
    p.add_argument("--dry-run", action="store_true")
    a = p.parse_args()

    picked = [s for s in scenarios() if not a.only or s["id"] in a.only.split(",")]
    run = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())
    print(f"run {run}: {len(picked)} mails to {a.to}")
    if a.dry_run:
        for s in picked:
            print(f"  {s['id']:<22} {s['sender']:<20} {s['subject']}  files={[f[0] for f in s.get('files', [])]}")
        return

    e = env(a.env)
    _, from_addr = email.utils.parseaddr(e["SMTP_FROM"])
    domain = from_addr.split("@")[1]
    manifest = {"run": run, "to": a.to, "sent": []}
    ctx = ssl.create_default_context()
    with smtplib.SMTP_SSL(e["SMTP_HOST"], int(e.get("SMTP_PORT", "465")), context=ctx) as smtp:
        smtp.login(e["SMTP_USERNAME"], e["SMTP_PASSWORD"])
        for i, s in enumerate(picked):
            m = EmailMessage()
            m["From"] = email.utils.formataddr((s["sender"], from_addr))
            m["To"] = a.to
            m["Subject"] = s["subject"]
            m["Date"] = email.utils.formatdate(localtime=False)
            m["Message-ID"] = email.utils.make_msgid(idstring=f"prtest.{s['id']}", domain=domain)
            m["X-PostRoom-Test"] = f"{run}:{s['id']}"
            m.set_content(s["body"])
            for name, mime, data in s.get("files", []):
                main_t, sub_t = mime.split("/", 1)
                m.add_attachment(data, maintype=main_t, subtype=sub_t, filename=name)
            smtp.send_message(m)
            manifest["sent"].append({"id": s["id"], "message_id": m["Message-ID"], "sent_at": int(time.time() * 1000), "expect": s["expect"]})
            print(f"  sent {s['id']:<22} {m['Message-ID']}")
            if i + 1 < len(picked):
                time.sleep(a.gap)

    os.makedirs(os.path.join(os.path.dirname(os.path.abspath(__file__)), "runs"), exist_ok=True)
    path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "runs", f"{run}.json")
    json.dump(manifest, open(path, "w"), indent=2)
    print(f"manifest: {path}\nnext: python3 -I check.py {path}   (give Gmail sync + agents a few minutes)")


if __name__ == "__main__":
    main()
