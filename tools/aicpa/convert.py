"""Converts the AICPA Annual Tax Compliance Kit's engagement letters (.docx) into Vibe Files
letter templates (ADDENDUM-annual-agreements.md, owner 2026-11-25, A3/A4/A16/A18).

    python -I tools/aicpa/convert.py <folder of the kit's .docx files>

Writes crates/vf-letters/assets/aicpa/<id>.json (one Template each) and tools/aicpa/manifest.json
(per file: every paragraph left out and why, and every fill-in left for the firm), which
crates/vf-letters' tests check the templates against.

What it does, per the kit's own "How to use this document":
- the cover ("Overview", "How to use this document"), the "SAMPLE ONLY" notice, footnotes and the
  CNA disclaimer at the end are left out;
- the kit's notes to the firm, whole paragraphs in parentheses such as "(On the rare occasion
  that the firm will initiate tax payments ... include the following provision ...)" and inline
  "(Optional ...)", are left out (A16); the optional (blue italic) provisions they introduce stay
  as ordinary text (A4);
- bracketed fill-ins with a field become the field ([CPA Firm] -> {Firm name}); the rest stay as
  written for the firm to fill in (Vibe Files refuses to send a letter that still has one, A17);
- the client address is left out (A14);
- the "INITIAL HERE" acknowledgement is left out and a {Terms and Conditions} paragraph stands
  before "Very truly yours," (A5): the Terms and Conditions template's text goes there;
- the firm signs with the partner's name printed (A7); the client's signature lines become
  signature blocks for the signers the letter names.
"""

import json
import os
import re
import sys
import zipfile
from xml.etree import ElementTree as ET

W = '{http://schemas.openxmlformats.org/wordprocessingml/2006/main}'
HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
OUT = os.path.join(REPO, 'crates', 'vf-letters', 'assets', 'aicpa')

IND = ['taxpayer', 'spouse']
ENT = ['officer']
BOTH = 'both'

# file stem (after "2026-") -> id, name the client sees, return types/services, kind, signers, doc type
FILES = {
    'individual-income-tax-return-engagement-letter-form-1040':
        ('aicpa-1040', 'Engagement letter: individual income tax return (Form 1040)', ['1040'], 'letter', IND),
    'c-corporation-engagement-letter-form-1120':
        ('aicpa-1120', 'Engagement letter: C corporation income tax return (Form 1120)', ['1120'], 'letter', ENT),
    's-corporation-tax-return-engagement-letter-form-1120s':
        ('aicpa-1120s', 'Engagement letter: S corporation income tax return (Form 1120-S)', ['1120-S'], 'letter', ENT),
    'partnership-engagement-letter-form-1065':
        ('aicpa-1065', 'Engagement letter: partnership return (Form 1065)', ['1065'], 'letter', ENT),
    'estate-and-trust-engagement-letter-form-1041':
        ('aicpa-1041', 'Engagement letter: estate or trust income tax return (Form 1041)', ['1041'], 'letter', ENT),
    'exempt-organization-engagement-letter-form-990':
        ('aicpa-990', 'Engagement letter: exempt organization return (Form 990)', ['990'], 'letter', ENT),
    'private-foundation-engagement-letter-form-990pf':
        ('aicpa-990pf', 'Engagement letter: private foundation return (Form 990-PF)', ['990-PF'], 'letter', ENT),
    'gift-tax-return-engagement-letter-form-709':
        ('aicpa-709', 'Engagement letter: gift tax return (Form 709)', ['709'], 'letter', ['taxpayer']),
    'estate-and-inheritance-engagement-letter-forms-706-8971':
        ('aicpa-706', 'Engagement letter: estate tax return (Forms 706 and 8971)', ['706'], 'letter', ENT),
    'employee-benefit-plan-engagement-letter-form-5500':
        ('aicpa-5500', 'Engagement letter: employee benefit plan return (Form 5500)', ['5500'], 'letter', ENT),
    'report-of-foreign-bank-and-financial-accounts-engagement-letter-fincen-form-114':
        ('aicpa-fbar', 'Engagement letter: foreign bank account report (FinCEN Form 114)', ['FBAR'], 'letter', BOTH),
    'amended-return-engagement-letter':
        ('aicpa-amended', 'Engagement letter: amended return', ['Amended return'], 'letter', BOTH),
    'tax-projections-engagement-letter':
        ('aicpa-projections', 'Engagement letter: tax projections', ['Tax projections'], 'letter', BOTH),
    'tax-consulting-engagement-letter':
        ('aicpa-consulting', 'Engagement letter: tax consulting', ['Tax consulting'], 'letter', BOTH),
    'audit-representation-engagement-letter':
        ('aicpa-audit-rep', 'Engagement letter: tax examination representation', ['Audit representation'], 'letter', BOTH),
    'sample-terms-and-conditions-addendum':
        ('aicpa-terms', 'Terms and Conditions', [], 'terms', []),
    'engagement-closure-letter':
        ('aicpa-closure', 'Engagement closure letter', [], 'closure', []),
}

FIELDS = [
    ('[Client & Spouse Names]', '{Client name}'),
    ('[Client Name]', '{Client name}'),
    ('[Name of Client]', '{Client name}'),
    ('[CPA Firm Name]', '{Firm name}'),
    ('[CPA Firm]', '{Firm name}'),
    ('[Accountant Name]', '{Partner name}'),
    ('[Accountant Title]', '{Partner title}'),
    ('[Authorized Client Representative Name and Title]', '{Signer name}, {Signer title}'),
    ('[Client Name or Client Representative]', '{Signer name}'),
    ('[Client Representative]', '{Signer name}'),
    ('[Title of Representative]', '{Signer title}'),
    ('[Executor or Trustee Name]', '{Signer name}'),
    ('[Executor Name]', '{Signer name}'),
    ('[Employee Benefit Plan Administrator]', '{Signer name}'),
    ('[Employee Benefit Plan Name]', '{Client name}'),
    ('[Name of Estate or Trust]', '{Client name}'),
    ('[Estate Name]', '{Client name}'),
    ('[Client Name’s]', '{Client name}’s'),
    ('[20XX]', '{Tax year}'),
    ('20[XX]', '{Tax year}'),
    ('20[YY]', '{Next year}'),
]
FILL_IN = re.compile(r'\[[^\[\]]*[A-Z0-9][^\[\]]*\]')
OPTIONAL_NOTE = re.compile(r'\s*\((?:Optional|optional)(?:[^()]|\([^()]*\))*\)')
SALUTATION = re.compile(r'^Dear \[[^\]]+\]:?$')


def text_of(p):
    """Paragraph text with **bold** markup; footnote references and italics dropped."""
    out = []
    for r in p.iter(W + 'r'):
        rp = r.find(W + 'rPr')
        bold = rp is not None and rp.find(W + 'b') is not None and \
            rp.find(W + 'b').get(W + 'val') not in ('0', 'false')
        t = ''
        for x in r:
            if x.tag == W + 't':
                t += x.text or ''
            elif x.tag == W + 'tab':
                t += ' '
            elif x.tag == W + 'br':
                t += '\n'
        if t:
            out.append((t, bold))
    # Bold runs that are only spaces or punctuation are not worth marking.
    s = ''
    for t, b in out:
        if b and t.strip() and t.strip() not in '.,:;':
            lead = t[:len(t) - len(t.lstrip())]
            trail = t[len(t.rstrip()):]
            s += lead + '**' + t.strip() + '**' + trail
        else:
            s += t
    return s.replace('****', '')


def plain(s):
    return s.replace('**', '').strip()


def style(p):
    ppr = p.find(W + 'pPr')
    if ppr is None:
        return '', False
    st = ppr.find(W + 'pStyle')
    num = ppr.find(W + 'numPr') is not None
    return (st.get(W + 'val') if st is not None else ''), num


def all_bold(s):
    t = s.strip()
    return t.startswith('**') and t.endswith('**') and t.count('**') == 2


def is_heading(s, st, listed):
    t = plain(s)
    if not t or listed:
        return False
    if st.startswith('Heading'):
        return True
    if len(t) > 80 or t[-1] in '.:;,?)' or t[0] in '[{' or t.startswith('Form '):
        return False
    return all_bold(s) or len(t) <= 60


def fields_in(s, para_text):
    for a, b in FIELDS:
        s = s.replace(a, b)
    if 'fee' in para_text.lower():
        s = s.replace('$[X]', '{Fee}')
    return s


def convert(path, stem):
    id_, name, types, kind, signers = FILES[stem]
    body = ET.fromstring(zipfile.ZipFile(path).read('word/document.xml')).find(W + 'body')
    items = []  # ('p', text, style, listed) | ('tbl', rows)
    for el in body:
        if el.tag == W + 'p':
            st, num = style(el)
            items.append(('p', text_of(el), st, num or st == 'ListParagraph'))
        elif el.tag == W + 'tbl':
            rows = []
            for tr in el.iter(W + 'tr'):
                rows.append([' '.join(plain(text_of(p)) for p in tc.iter(W + 'p')).strip()
                             for tc in tr.findall(W + 'tc')])
            items.append(('tbl', rows))
    dropped = []
    # Where the letter starts: the first "[Date]" (letters) or the addendum's own title (terms).
    if kind == 'terms':
        start = next(i for i, x in enumerate(items)
                     if x[0] == 'p' and plain(x[1]).endswith('Terms and Conditions Addendum')
                     and plain(x[1]).startswith('[CPA Firm]'))
    else:
        start = next(i for i, x in enumerate(items) if x[0] == 'p' and plain(x[1]) == '[Date]')
    # The end: the separator before the CNA disclaimer (a letter can have another mid-way).
    cna = next(i for i, x in enumerate(items) if x[0] == 'p' and 'produced and presented by CNA' in x[1])
    end = max(i for i, x in enumerate(items[:cna]) if x[0] == 'p' and plain(x[1]).startswith('* * *'))
    dropped.append({'why': 'cover and notes before the letter', 'paragraphs': start})
    dropped.append({'why': 'CNA disclaimer after the letter', 'paragraphs': len(items) - end})
    blocks = []
    closing = False

    def p(text):
        blocks.append({'kind': 'paragraph', 'text': text})

    i = start
    while i < end:
        x = items[i]
        i += 1
        if x[0] == 'tbl':
            rows = [[fields_in(c, c) for c in r] for r in x[1] if any(c for c in r)]
            if rows:
                blocks.append({'kind': 'table', 'rows': rows})
            continue
        _, s, st, listed = x
        t = plain(s)
        if not t:
            continue
        if t.startswith('* * *'):
            continue
        if t.startswith('SAMPLE ONLY'):
            dropped.append({'why': 'sample notice', 'text': t})
            continue
        if t.startswith('(') and t.endswith(')'):
            dropped.append({'why': 'note to the firm (A16)', 'text': t})
            continue
        if re.fullmatch(r'\[[^\]]*Address\]', t):
            dropped.append({'why': 'client address (A14)', 'text': t})
            continue
        if t.startswith('By initialing below') or t.startswith('INITIAL HERE'):
            dropped.append({'why': 'initials: the Terms and Conditions are part of the letter (A5)', 'text': t})
            continue
        if t == 'Very truly yours,':
            # The ending: Terms and Conditions, the firm's signature, the client's.
            rest = [plain(y[1]) for y in items[i:end] if y[0] == 'p' and plain(y[1])]
            dropped.append({'why': 'signature lines (replaced by signature blocks)', 'text': ' | '.join(rest)})
            if kind != 'closure':
                p('{Terms and Conditions}')
            p('Very truly yours,')
            p('{Firm name}')
            p('/s/ {Partner name}')
            p('{Partner title}')
            if signers:
                p('ACCEPTED:')
                if signers == BOTH:
                    blocks.append({'kind': 'signature', 'role': 'taxpayer', 'only_for': ['Individual']})
                    blocks.append({'kind': 'signature', 'role': 'spouse', 'only_for': ['Individual']})
                    blocks.append({'kind': 'signature', 'role': 'officer', 'only_for': ['Entity']})
                else:
                    for r in signers:
                        blocks.append({'kind': 'signature', 'role': r})
            closing = True
            break
        before = s
        s = OPTIONAL_NOTE.sub('', s)
        if plain(s) != plain(before):
            dropped.append({'why': 'inline note to the firm (A16)', 'text': re.findall(OPTIONAL_NOTE, before)})
        s = s.rstrip()
        if t == '[Date]':
            p('{Date}')
            continue
        if SALUTATION.match(t):
            p('Dear {Signer first names}:')
            continue
        s = fields_in(s, t)
        if is_heading(s, st, listed):
            blocks.append({'kind': 'heading', 'text': plain(s)})
        elif listed:
            if blocks and blocks[-1]['kind'] == 'list' and items[i - 2][0] == 'p' and items[i - 2][3]:
                blocks[-1]['items'].append(s.strip())
            else:
                blocks.append({'kind': 'list', 'items': [s.strip()]})
        else:
            p(s.strip())
    if kind != 'terms' and not closing:
        raise SystemExit(f'{stem}: no "Very truly yours," found')
    def texts(b):
        return [b.get('text', '')] + b.get('items', []) + [c for r in b.get('rows', []) for c in r]
    fill = sorted(set(m for b in blocks for s in texts(b) for m in FILL_IN.findall(s)))
    tpl = {
        'id': id_,
        'version': 0,
        'name': name,
        'return_types': types,
        'kind': kind,
        'doc_type': {'terms': '', 'closure': 'Correspondence'}.get(kind, 'Engagement letter'),
        'letterhead': True,
        'default_ticked': kind == 'letter',
        'sample': True,
        'blocks': blocks,
    }
    return tpl, {'id': id_, 'file': os.path.basename(path), 'blocks': len(blocks),
                 'words': sum(len(json.dumps(b, ensure_ascii=False).split()) for b in blocks),
                 'fill_ins': fill, 'dropped': dropped}


def main():
    src = sys.argv[1]
    os.makedirs(OUT, exist_ok=True)
    manifest = []
    seen = set()
    for f in sorted(os.listdir(src)):
        if not f.endswith('.docx'):
            continue
        stem = re.sub(r'^\d{4}-', '', f[:-5])
        if stem not in FILES:
            raise SystemExit(f'{f}: not a kit file this converter knows')
        tpl, m = convert(os.path.join(src, f), stem)
        seen.add(stem)
        with open(os.path.join(OUT, tpl['id'] + '.json'), 'w', encoding='utf-8', newline='\n') as o:
            json.dump(tpl, o, ensure_ascii=False, indent=1)
            o.write('\n')
        manifest.append(m)
        print(f"{tpl['id']:<20} {m['blocks']:>4} blocks {m['words']:>6} words {len(m['fill_ins']):>3} fill-ins")
    missing = set(FILES) - seen
    if missing:
        raise SystemExit(f'missing kit files: {sorted(missing)}')
    with open(os.path.join(HERE, 'manifest.json'), 'w', encoding='utf-8', newline='\n') as o:
        json.dump(manifest, o, ensure_ascii=False, indent=1)
        o.write('\n')


if __name__ == '__main__':
    main()
