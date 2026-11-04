# The ten-key tape

An adding-machine tape under the page in the Viewer: type amounts or pick them off the PDF, take subtotals and totals, keep the tape with the document, and paste it onto the page.

## Opening the tape

In the [Viewer](help:viewer), click **Tape** in the toolbar. The tape opens under the page: lines on the left, keypad on the right. Click **Tape** again to hide it. You can name the tape in the box beside **Tape**.

## The keys

The tape works like a desk adding machine, not like a pocket calculator. Amounts are dollars and cents.

- Digits, **00** and **.** type an amount. **Del** removes the last digit.
- **+** and **−** post the amount to the running total, one tape line each.
- **S** prints the subtotal and keeps adding.
- **T** prints the total, adds it to the grand total, and starts again at zero.
- **GT** prints the grand total of your totals and clears it.
- **CE** clears the amount being typed. **Clear** clears the whole tape and its totals.
- **±** flips the sign of the amount being typed.
- **×** and **÷** work business-calculator style: `12 × 3 +` prints `12.00 ×`, `3.00 =`, then posts `36.00 +`. Press **=** instead of **+** to keep the result as the next amount without posting it.

Results of **×** and **÷** are rounded to the cent (half a cent rounds away from zero), so `10 ÷ 3 +` posts `3.33`. **S** and **T** finish a waiting **×** or **÷** first, just as **+** does. Dividing by zero is refused with "Cannot divide by zero" and nothing is posted.

You can also use the keyboard when no box has the cursor: digits, `.`, `+`, `-`, `*`, `/`, `=`, Enter for **+**, Backspace for **Del**, Delete for **CE**, Ctrl+Z to undo and Ctrl+Shift+Z to redo.

## Picking amounts off the page

1. Click **Pick from page**. Amounts on the page get a dotted underline.
2. Click an amount: it is added. Click it again: it flips to a subtraction.
3. Or drag a box: every amount in it is added, top to bottom.

Picked lines get a numbered marker on the page. Click a picked amount on the tape to show where it came from. Esc or **Keypad** goes back to typing. Plain numbers without a point, comma or dollar sign, such as years and box numbers, are not offered.

With **Labels from box captions** on, each picked line is labelled with the words beside or above the amount on the form, plus its page. You can edit any label on the tape.

## Undo, Redo and History

**Undo** and **Redo** step back and forward through every change to the tape. **Copy** puts the tape on the clipboard, as text and as columns for Excel.

**Save tape** keeps the tape with the document. **History** lists the tapes kept and the tapes pasted on its pages; choosing one loads it (Undo brings your working tape back).

## Pasting the tape onto the page

1. Choose **Include labels & initials** to print the labels and a footer with your initials, today's date and the **Note**, if any.
2. Choose **Link entries to page** to keep where each picked amount was.
3. Click **Paste to page**, then click on the page where the tape goes.

The pasted tape waits as an unsaved annotation: **Save** writes it into the file as a new version, **Save as…** into a new document. See [annotations](help:annotations).
