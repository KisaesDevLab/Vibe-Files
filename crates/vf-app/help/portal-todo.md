# To-do lists on the portal

How to ask a client for documents and answers through your firm's portal, see what they sent, and keep a record of it.

## Turning them on

To-do lists have two switches, and both start off:

- **On the portal:** an admin ticks **Turn on to-do lists** in the portal's staff page, **Settings › To-do lists**. While it is off, clients see no To do and no list can be sent.
- **In Vibe Files:** tick **To-do lists** in **Admin › Portal** and press **Save**. It is kept with the cabinet's portal settings, so every PC follows it. While it is off, the client's **To-do lists…** button is hidden and **Admin › To-do templates** says they are off.

It needs Vibe Portal 0.5 or later.

## Sending a list

Select a client in Browse and press **To-do lists…** in its details. Press **+ New list**, or choose **From a template…**, then:

- give the list a title, a note to the client if you like, and a due day (2027-03-15);
- add the items: each asks for **Documents** (the client sends files) or **An answer** (the client types one). Untick **Required** for an item that is optional. A **Doc type** tells Intake what the files are, when it files them;
- choose whether the portal reminds the client by itself (every 3, 7 or 14 days while items are open);
- choose who is **Assigned to** it: that staff member gets an email when the client answers (answers within a few minutes come in one email), and once when every required item is answered;
- leave **Tell the client's people now** ticked to send them an email or text (as each chose) saying how many items there are and when they are due. The message never says what the items are.

## What the client does

On the portal, the client sees **To do** at the top of their page. For each item they can send files, type an answer, or say it does not apply (with a reason if they like). They can answer again or take a file back until the firm has it; every answer is kept, the latest counts.

Files sent for an item arrive in the **Portal inbox** like any other upload, named for the client, and Intake files them with the item's doc type.

## The record of every answer

The portal keeps each answer with its date and time, the internet address it came from, the browser, and the person signed in (their name, email and phone as they were then). Answers are never changed or deleted.

Open a list to see every answer with these details. **Download report…** saves the list as a PDF report with all of them (the answers are also attached to the PDF as data). **File report in the client folder** puts the same PDF in the Portal inbox, named for the client, so Intake files it.

Vibe Files also keeps a copy of each list it reads in the client's folder, `_Vibe\todo\<list>.jsonl`: plain JSON, one line each time the list was found changed. When the portal cannot be reached, **To-do lists…** shows these copies.

## Changing, closing and deleting

- **Edit** changes the title, note, due day, reminders, who is assigned, and the items. An item the client has answered keeps its wording and kind.
- **Remind now** tells the client's people what is still open (at most 3 times a day).
- **Close** ends the list: the client sees it but can no longer answer. **Open again** reopens it.
- **Cancel list** takes it away from the client; its answers are kept.
- **Delete** removes a list nobody has answered. A list with answers cannot be deleted: cancel it instead.

## Templates

**Admin › To-do templates** keeps lists you send often (the year-end documents for a 1040, a loan package). **Send to clients…** sends one to many clients at once: tick them, set the due day, and send. Each client gets a list of their own. The portal's staff page has the same templates.

Every change made here is in the audit trail.
