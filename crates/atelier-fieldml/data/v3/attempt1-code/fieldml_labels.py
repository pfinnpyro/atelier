"""Geometry-only printed label and section association, independent of gold data."""
import re
from collections import defaultdict
import pymupdf as fitz


def clean(text):
    text=re.sub(r'[_…]{2,}', ' ', text)
    return ' '.join(text.replace('||',' ').split()).strip(' :')


def words(page):
    """Split at actual writing lines even when the PDF stores Name____ as one word."""
    out=[];line_id=0
    for block in page.get_text('rawdict')['blocks']:
        for line in block.get('lines',[]):
            token=[]
            def flush():
                if not token:return
                text=''.join(c['c'] for c in token)
                if any(c.isalnum() for c in text):
                    rect=fitz.Rect(token[0]['bbox'])
                    for c in token[1:]:rect|=fitz.Rect(c['bbox'])
                    out.append((*list(rect),text,line_id))
                token.clear()
            for span in line['spans']:
                for c in span.get('chars',[]):
                    if c['c'].isspace() or c['c'] in '_…':flush()
                    else:token.append(c)
            flush();line_id+=1
    return out


ROLE=re.compile(r'^(?:name of company|address of company|residential address|mailing address\s*\(if different|alternate address|secondary address|dependants?|dependents?|immediate family members|applicant.*(?:children|parents)|family information|application information|(?:current|previous|prior) employment|personal (?:information|data|details)|contact (?:information|details)|applicant(?: information)?|employee information|patient (?:info|information|demographics)|participant information|residence$|present address|current address|previous (?:address|residence)|prior (?:address|residence)|business$|employment(?: information| history| info)?$|(?:current|previous|present or most recent|second most recent|third most recent) employer|responsible party|guarantor|primary insurance|insurance information|(?:personal |professional )?references?$|referees?|.*in the case of emergency.*contact|emergency contacts?(?: information)?$|parent\s*/\s*guardian|primary guardian|secondary guardian|name of (?:parent|guardian|spouse|insured)|spouse(?: information)?$|landlord|owner/manager|veterinarian|clinic(?: information)?$|sponsor|propos(?:er|ing sponsor)|preparer|translator|physician|hospital|medical facility|school information|education$|post.secondary plans|office use only)',re.I)


def section(page, rect):
    r=fitz.Rect(rect);candidates=[]
    for block in page.get_text('dict')['blocks']:
        for line in block.get('lines',[]):
            text=''.join(s['text'] for s in line['spans'])
            text=clean(re.split(r'_{2,}',text)[0])
            text=re.sub(r'^(?:(?:[ivxlcdm]+|[0-9]+)\s*[).]\s*)+','',text,flags=re.I)
            text=re.sub(r'^particulars of\s+','',text,flags=re.I)
            b=fitz.Rect(line['bbox'])
            if text.lower().startswith('office use') and r.y0-b.y0>100:continue
            if b.y0<=r.y0+2 and b.x0<=r.x0+20 and len(text)<130 and ROLE.search(text):
                candidates.append((b.y0,ROLE.search(text).group(0)))
    return max(candidates,default=(0,''))[1]


def extract(page, field):
    r=fitz.Rect(field['rect']);by_line=defaultdict(list)
    for w in words(page):by_line[w[5]].append(w)
    candidates=[]
    for line in by_line.values():
        line.sort(key=lambda w:w[0])
        left=[w for w in line if w[2]<=r.x0+2 and abs((w[1]+w[3])/2-(r.y0+r.y1)/2)<=max(16,r.height/2+4)]
        if left:
            selected=[left[-1]]
            for w in reversed(left[:-1]):
                if selected[0][0]-w[2]>18:break
                selected.insert(0,w)
            gap=r.x0-selected[-1][2]
            if gap<220:
                dy=abs(sum((w[1]+w[3])/2 for w in selected)/len(selected)-(r.y0+r.y1)/2)
                candidates.append((gap*.15+dy*.25,selected,'left'))
        above=[w for w in line if r.x0-3<=w[0]<r.x1 and -3<=r.y0-w[3]<=20]
        if above:candidates.append((3+abs(r.y0-max(w[3] for w in above)),above,'above'))
        below=[w for w in line if r.x0-3<=w[0]<r.x1 and 0<=w[1]-r.y1<=18]
        if below:candidates.append((4+min(w[1] for w in below)-r.y1,below,'below'))
    label='';evidence=[]
    if candidates:
        def association_score(candidate):
            score,items,_=candidate
            text=' '.join(w[4] for w in items)
            latin=re.findall(r'[A-Za-z]+',text)
            # A title-choice decoration or untranslated script next to the
            # writing line should not outrank its English label just above it.
            if not latin or all(word.lower() in ['mr','mrs','ms','miss','mdm','dr'] for word in latin):score+=35
            return score
        _,selected,kind=min(candidates,key=association_score)
        label=clean(' '.join(w[4] for w in selected))
        if kind=='left' and label.lower() in ['full name','address']:
            children=[c for c in candidates if c[2]=='below' and c[0]<=10 and clean(' '.join(w[4] for w in c[1])).lower() in ['first','last','m.i.','mi','street address','apt/unit #','city','state','zip code']]
            if children:
                _,selected,_=min(children,key=lambda c:c[0]);label=clean(' '.join(w[4] for w in selected))
        evidence=[list(w[:4]) for w in selected]
    tooltip=field.get('tooltip','')
    if tooltip and not re.fullmatch(r'(?:Text|NumericField|untitled|--)[\d\[\]]*',tooltip,re.I):
        label=clean(tooltip);evidence=[]
    if not label:label=clean(field.get('native_name','')) or 'Unlabeled field'
    # Explanatory mailing/newsletter prose is not the field's semantic label.
    # Keep date/name-order and conditional '(if different)' hints intact.
    label=re.split(r'\s*\((?:you will|we will|please|for example|e\.g\.)\b',label,flags=re.I)[0].strip()
    # Name-order instructions can appear below a writing line rather than in /TU.
    if re.search(r'\bname\b',label,re.I) and not re.search(r'\b(?:first|last|middle)\b',label,re.I):
        for block in page.get_text('dict')['blocks']:
            for line in block.get('lines',[]):
                text=clean(''.join(s['text'] for s in line['spans']))
                b=fitz.Rect(line['bbox'])
                if -20<=b.y0-r.y1<=65 and re.search(r'last name\s*[,/]?\s*first name',text,re.I):
                    label+=' (Last name, First name)'
    return label,section(page,r),evidence


def label_rects(page, anchor):
    """Locate a complete phrase across PDF spans, not one rectangle per font.

    PyMuPDF search_for can return several rectangles for one phrase when each
    glyph uses a different font/span. Match raw characters and union their boxes.
    """
    pattern=re.escape(anchor).replace(r'\ ',r'\s*')
    if anchor and anchor[0].isalnum():pattern=r'(?<![^\W_])'+pattern
    if anchor and anchor[-1].isalnum():pattern+=r'(?![^\W_])'
    regex=re.compile(pattern,re.I)
    hits=[]
    for block in page.get_text('rawdict')['blocks']:
        for line in block.get('lines',[]):
            chars=[c for span in line['spans'] for c in span.get('chars',[])]
            text=''.join(c['c'] for c in chars)
            for match in regex.finditer(text):
                rect=fitz.Rect(chars[match.start()]['bbox'])
                for c in chars[match.start()+1:match.end()]:rect|=fitz.Rect(c['bbox'])
                hits.append(rect)
    return sorted(hits,key=lambda r:(round(r.y0),r.x0))
