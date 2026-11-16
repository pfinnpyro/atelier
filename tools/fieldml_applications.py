#!/usr/bin/env python3
"""Build an independently annotated application benchmark and geometry inputs.
No personal/user files: only sources declared in applications.json are opened.
"""
import csv
import hashlib
import json
import re
from collections import defaultdict
from pathlib import Path
import pymupdf as fitz

ROOT = Path(__file__).resolve().parents[1]
DATA = ROOT / 'crates/atelier-fieldml/data'
APP = DATA / 'sources/applications'


def clean(s):
    return ' '.join(s.replace('||',' ').split())


def label_from_geometry(page, rect):
    """Associate an existing widget/annotated blank slot with nearby printed words.
    Uses no gold labels, classes, per-source mappings, or expected profile values.
    """
    r = fitz.Rect(rect)
    candidates = []
    by_line = defaultdict(list)
    for w in page.get_text('words'):
        if not re.search(r'[A-Za-z]', w[4]): continue
        by_line[w[5:7]].append(w)
    for words in by_line.values():
        words.sort(key=lambda w: w[0])
        # Same-line labels sit immediately to the left of a writable area.
        left = [w for w in words if w[2] <= r.x0+2 and abs((w[1]+w[3])/2-(r.y0+r.y1)/2) < max(9,r.height/2+2)]
        if left:
            group = [left[-1]]
            for w in reversed(left[:-1]):
                if group[0][0]-w[2] > 18: break
                group.insert(0,w)
            gap = r.x0-group[-1][2]
            if gap < 90:
                candidates.append((gap*.15,clean(' '.join(w[4] for w in group)),group))
        # Box labels often sit inside the box top or immediately above it.
        above = [w for w in words if r.x0-4 <= w[0] < r.x1 and -4 <= r.y0-w[3] <= 20]
        if above:
            gap = abs(r.y0-max(w[3] for w in above))
            candidates.append((3+gap,clean(' '.join(w[4] for w in above)),above))
    candidates = [c for c in candidates if 1 < len(c[1]) < 160]
    if not candidates: return '', []
    _,text,words = min(candidates,key=lambda c:c[0])
    return text, [list(w[:4]) for w in words]


# Generic section/role boundaries, fixed before held-out predictions are read.
# Inline ownership markers are necessary in forms without styled headings.
SECTION = re.compile(r'^(?:responsible party$|patient info$|patient information|primary insurance|insurance information|employment info|business$|residence$|sponsor\b|references?$|personal information|contact information|family.*housing|family.*household|veterinar|professional references|personal references|emergency contact|.*in the case of emergency.*contact|parent / guardian information|parent/guardian|primary guardian|secondary guardian|previous address|current address|current employer|previous employer|present or most recent employer|second most recent employer|third most recent employer|applicant|participant information)',re.I)


def section_from_geometry(page, rect):
    r=fitz.Rect(rect); choices=[]
    for block in page.get_text('dict')['blocks']:
        for line in block.get('lines',[]):
            text=clean(''.join(s['text'] for s in line['spans']))
            # Remove writable underscores and values following them.
            short=re.split(r'_{2,}',text)[0].strip(' :')
            b=fitz.Rect(line['bbox'])
            if b.y0 <= r.y0+2 and b.x0 <= r.x0+20 and SECTION.search(short):
                if len(short)<120:
                    choices.append((b.y0,short))
    return max(choices,default=(0,''))[1]


def extract(page, field):
    label, evidence=label_from_geometry(page,field['rect'])
    # Readable /TU is a legitimate input, but never use the gold annotation.
    tooltip=field.get('tooltip','')
    if tooltip and not re.fullmatch(r'(?:Text|NumericField|untitled|--)[\d\[\]]*',tooltip,re.I):
        label=tooltip
        evidence=[]
    if not label: label=field.get('native_name','') or 'Unlabeled field'
    return clean(label), section_from_geometry(page,field['rect']), evidence


def build():
    manifest=json.loads((DATA/'applications.json').read_text(encoding='utf-8'))
    annotations=[]
    for source in manifest:
        if source.get('sha256'):
            actual=hashlib.sha256((APP/(source['id']+'.pdf')).read_bytes()).hexdigest()
            if actual!=source['sha256']: raise ValueError(('source changed: re-review annotations',source['id']))
    docs={s['id']:fitz.open(APP/(s['id']+'.pdf')) for s in manifest}
    def manual(source,page,anchor,klass,section='',occ=0,label=None,box=None):
        p=docs[source][page-1]
        hits=label_rects(p,anchor) if 'label_rects' in globals() else sorted(p.search_for(anchor),key=lambda r:(round(r.y0),r.x0))
        if anchor.isalpha() and len(anchor)<=4:
            hits=[h for h in hits if any(w[4].strip(':_ ').lower()==anchor.lower() and fitz.Rect(w[:4]).intersects(h) for w in p.get_text('words'))]
        if occ>=len(hits): raise ValueError((source,page,anchor,occ,len(hits)))
        b=fitz.Rect(box) if box is not None else hits[occ]; rect=[b.x1+2,b.y0,b.x1+82,b.y1]
        rect[2]=min(rect[2],p.rect.width-12)
        annotations.append(dict(source=source,page=page,field=f'p{page}-{len(annotations):03}',
            klass=klass,label=label or anchor,section=section,rect=rect,
            label_rect=list(b),native_name='',tooltip='',origin='manually located printed blank slot'))
    def widgets(source,positive,context):
        for pi,page in enumerate(docs[source]):
            for widget in page.widgets() or []:
                if widget.field_type!=fitz.PDF_WIDGET_TYPE_TEXT: continue
                name=widget.field_name
                klass,label=positive.get(name,('Other',widget.field_label or name))
                section=context(pi+1,widget)
                annotations.append(dict(source=source,page=pi+1,field=name,klass=klass,
                    label=clean(label),section=section,rect=list(widget.rect),native_name=name,
                    tooltip=widget.field_label or '',origin='native PDF text widget'))
    widgets('employment',{
        'Last Name First Middle':('NameLastFirst','Last Name First Middle'),
        'Present Street Address':('AddressLine1','Present Street Address'),
        'Phone Number':('Phone','Phone Number'), 'City State Zip':('CityStateZip','City, State, Zip'),
        'Email AddressPosition you are applying for':('Email','Email Address'),
        'Your Name':('FullName','Your Name'),
    },lambda page,w:'PRESENT OR MOST RECENT EMPLOYER' if page==2 and w.rect.y0<210 else
        'SECOND MOST RECENT EMPLOYER' if page==2 and w.rect.y0<360 else
        'THIRD MOST RECENT EMPLOYER' if page==2 and w.rect.y0<515 else
        'REFERENCES' if page==2 else 'EDUCATION' if page==3 else '')
    widgets('adoption-vet',{
        'Full Name':('FullName','Full Name'),'Address':('AddressWhole','Address'),
        'DayŸme Phone':('Phone','Daytime Phone'),'Evening Phone':('Phone','Evening Phone'),
        'Email Address':('Email','Email Address'),
    },lambda page,w:'Contact Information' if page==1 and w.rect.y0<300 else
        'Family & Housing' if page==1 else 'Veterinarian Information' if page==2 and w.rect.y0>420 else
        'References' if page==3 and 440<w.rect.y0<590 else '')
    rotary_positive={1:('FullName','Prospective Member Name'),2:('BirthDate','Birthday'),
        12:('AddressWhole','Residence Address'),13:('Phone','Residence Telephone Number'),
        14:('Phone','Cell Phone Number'),15:('Email','Email')}
    rotary_labels=['Prospective Member Name','Birthday','Name of Spouse','Spouse Birthday','Anniversary',
        'Children & Birthdays','Business Name/Former employer','Position or Title','Business Address',
        'Business Telephone Number','Email','Residence Address','Residence Telephone Number',
        'Cell Phone Number','Email','Club name','from','to','RI ID']
    for pi,page in enumerate(docs['membership-rotary']):
        for i,w in enumerate(page.widgets() or [],1):
            if w.field_type!=fitz.PDF_WIDGET_TYPE_TEXT:continue
            k,label=rotary_positive.get(i,('Other',rotary_labels[i-1]))
            section='Personal Information (To be completed by Applicant)' if i<7 else 'Business' if i<12 else 'Residence'
            annotations.append(dict(source='membership-rotary',page=pi+1,field=w.field_name,klass=k,
                label=label,section=section,rect=list(w.rect),native_name=w.field_name,tooltip='',origin='native PDF text widget'))
    # Complete selected printed groups, including ambiguous repeated labels.
    for anchor,k in [('Name','FullName'),('Address','AddressWhole'),('City/State/zip','CityStateZip'),('Phone','Phone'),('Email','Email')]:
        manual('volunteer-freedom',1,anchor,k)
    for anchor,k in [('FIRST','FirstName'),('MI','MiddleInitial'),('LAST','LastName'),('Current address:','AddressLine1'),('City','City'),('State','State'),('Zip code','Zip'),('Former address:','Other'),('Date of birth:','BirthDate'),('Maiden name:','Other'),('Alias','Other')]:
        manual('volunteer-freedom',2,anchor,k,'APPLICANT')
    for anchor,k in [('Name','FullName'),('Home Phone','Phone')]:manual('volunteer-freedom',8,anchor,k,'',occ=0)
    # Table labels represent repeated third-party slots, never applicant data.
    for anchor in ['Emergency Contact Name','Relationship','Phone Number','Name of Children in School','Name of School','School’s Phone Number']:
        manual('volunteer-freedom',8,anchor,'Other','Emergency Contact Information')
    # Rental: all labels in first two pages; residence and employment role boundaries.
    rental=docs['rental-taylor']
    for pi,p in enumerate(rental):
        if pi>1:break
        for block in p.get_text('dict')['blocks']:
            for line in block.get('lines',[]):
                text=clean(''.join(s['text'] for s in line['spans']))
                b=fitz.Rect(line['bbox'])
                if len(text)>55 or ':' not in text:continue
                if pi==0 and 270<b.y0<310:
                    k={'Name:':'FullName','Phone:':'Phone','Email:':'Email'}.get(text,'Other');section=''
                elif pi==0 and 320<b.y0<360:
                    k={'Current Address:':'AddressLine1','City:':'City','State:':'State','Zip Code:':'Zip'}.get(text,'Other');section='Current Address'
                else:
                    k='Other';section='EMERGENCY CONTACT' if pi==1 else 'Previous Address' if 450<b.y0<579 else 'Current Employer' if 579<=b.y0<665 else 'Previous Employer' if b.y0>=665 else 'Landlord' if 400<b.y0<450 else 'Office use only' if b.y0<110 else ''
                manual('rental-taylor',pi+1,text,k,section,box=list(b))
    # Volunteer page 1 primary person and page 3 primary/secondary emergency contacts.
    for anchor,k in [('Full Name:','FullName'),('Date:','Other'),('Current Address:','AddressLine1'),('City:','City'),('State:','State'),('Zip:','Zip'),('Year moved in:','Other'),('SSN #','Other'),('Birth Date:','BirthDate'),('City/State of Birth:','Other'),('Home Phone:','Phone'),('Cell Phone:','Phone'),('Work:','Other'),('Email Address:','Email'),('Best way to contact you:','Other')]:
        manual('volunteer-turning',1,anchor,k)
    manual('volunteer-turning',3,'YOUR NAME:','NameLastFirst','',label='Your name (last name, first name)')
    for anchor in ['NAME:','RELATIONSHIP TO YOU:','MAILING ADDRESS:','EMAIL ADDRESS:','HOME PHONE NUMBER:','CELL PHONE NUMBER:']:
        for occ in [0,1]:
            # NAME: also matches YOUR NAME; use exact word matching at this page via occurrence.
            n=occ+1 if anchor=='NAME:' else occ
            manual('volunteer-turning',3,anchor,'Other','PRIMARY CONTACT' if occ==0 else 'SECONDARY CONTACT',occ=n)
    # Scholarship applicant and parent/guardian/education negatives.
    for anchor,k,section in [
        ('Applicable Scholarship:','Other','PERSONAL DATA'),("Candidate's Name:",'FullName','PERSONAL DATA'),
        ('Mailing Address:','AddressWhole','PERSONAL DATA'),('School:','Other','PERSONAL DATA'),
        ('Social Insurance Number:','Other','PERSONAL DATA'),('PEN Number:','Other','PERSONAL DATA'),
        ('Name of Parent/Guardian:','Other','PERSONAL DATA'),('Address (if different than above):','Other','Parent/Guardian'),
        ('Mother’s Place of Employment:','Other','PERSONAL DATA'),('Father’s Place of Employment:','Other','PERSONAL DATA'),
        ('Union/Club Affiliation of Parent:','Other','PERSONAL DATA'),('Name of Post-Secondary Institution:','Other','POST-SECONDARY PLANS'),
        ('Proposed Field of Study:','Other','POST-SECONDARY PLANS'),('Signature of Applicant','Other',''),('Date','Other','')]:
        manual('scholarship-sd91',1,anchor,k,section)
    # Patient profile is the patient, not the responsible party or policyholder.
    for anchor,k,occ,section in [
        ('First Name:','FirstName',0,''),('Last Name:','LastName',0,''),('Middle Int:','MiddleInitial',0,''),
        ('First Name:','Other',1,'Responsible Party'),('Last Name:','Other',1,'Responsible Party'),('Middle Int:','Other',1,'Responsible Party'),
        ('Address:','Other',0,'Responsible Party'),('Address 2:','Other',0,'Responsible Party'),
        ('City, State, Zip Code:','Other',0,'Responsible Party'),('Home Phone:','Other',0,'Responsible Party'),
        ('Cell Phone:','Other',0,'Responsible Party'),('Birth Date:','Other',0,'Responsible Party'),
        ('First Name:','FirstName',2,'Patient Info'),('Last Name:','LastName',2,'Patient Info'),
        ('Zip Code:','Zip',1,'Patient Info'),('Home Phone:','Phone',1,'Patient Info'),
        ('Cell Phone:','Phone',1,'Patient Info'),('Work Phone:','Other',1,'Patient Info'),
        ('Birth Date:','BirthDate',1,'Patient Info'),('E-Mail Address:','Email',0,'Patient Info'),
        ('Name of insured:','Other',0,'Primary Insurance Information'),('Insured Date of Birth','Other',0,'Primary Insurance Information'),
        ('Employer:','Other',0,'Primary Insurance Information'),('Ins Company:','Other',0,'Primary Insurance Information'),
        ('Address:','Other',2,'Primary Insurance Information'),('Address:','Other',3,'Primary Insurance Information'),
        ('City, State Zip','Other',0,'Primary Insurance Information'),('City, State, Zip','Other',1,'Primary Insurance Information')]:
        manual('patient-ripley',1,anchor,k,section,occ=occ)
    # Camp profile is the named first participant. Every other person stays blank.
    for anchor,k in [('Participant Name:','FullName'),('Age:','Other'),('D.O.B.','BirthDate'),('Grade in Fall 2025:','Other'),('School Currently Attending:','Other'),('Address:','AddressLine1'),('City:','City'),('Zip Code:','Zip')]:
        manual('camp-pearce',1,anchor,k)
    for anchor in ['Parent/Guardian Name 1:','Parent/Guardian Name 2:','Email (required):','Home Phone:','Cell Phone:','D.O.B:']:
        for occ in (range(2) if anchor in ['Email (required):','Home Phone:','Cell Phone:','D.O.B:'] else [0]):
            manual('camp-pearce',1,anchor,'Other','Parent/Guardian',occ=occ)
    for anchor in ['Name of Emergency Contact:','Relationship to Participant:','Phone:','Parent/Guardian Signature:']:
        manual('camp-pearce',1,anchor,'Other','EMERGENCY CONTACT INFORMATION',occ=4 if anchor=='Phone:' else 0)
    # YMCA first participant is the profile target, not the second child or adults.
    for anchor,k in [('PARTICIPANT’S NAME (1)','FullName'),('DOB','BirthDate'),('PARTICIPANT’S NAME (2)','Other')]:
        manual('camp-ymca',1,anchor,k,'PARTICIPANT INFORMATION')
    for anchor,occ in [('Name:',0),('Name:',1),('Email:',0),('Email:',1),('Home Phone:',0),('Home Phone:',1),('Cell Phone:',0),('Cell Phone:',1),('Address:',0),('Address:',1),('City:',0),('City:',1),('State:',0),('State:',1),('Zip:',0),('Zip:',1)]:
        manual('camp-ymca',2,anchor,'Other','PARENT / GUARDIAN INFORMATION',occ=occ)
    for anchor in ['Physician:','Phone Number:','Address:','Hospital:','Insurance Company Name:','Policy #:']:
        manual('camp-ymca',3,anchor,'Other','AUTHORIZATION FOR EMERGENCY MEDICAL CARE')
    # I-9 initial employee section and all employer/preparer fields. Later
    # conditional rehire blocks are excluded as they require workflow state.
    i9pos={'Last Name (Family Name)':'LastName','First Name Given Name':'FirstName',
        'Employee Middle Initial (if any)':'MiddleInitial','Address Street Number and Name':'AddressLine1',
        'Apt Number (if any)':'AddressLine2','City or Town':'City','ZIP Code':'Zip',
        'Date of Birth mmddyyyy':'BirthDate','Employees E-mail Address':'Email','Telephone Number':'Phone'}
    for pi,p in enumerate(docs['i9']):
        if pi not in [0,2]:continue
        for w in p.widgets() or []:
            if w.field_type!=fitz.PDF_WIDGET_TYPE_TEXT:continue
            name=w.field_name
            # Supplement A repeats employee name at top; those are supported.
            klass=i9pos.get(name,'Other')
            if pi==2 and 'from Section 1' in name:
                klass='LastName' if name.startswith('Last') else 'FirstName' if name.startswith('First') else 'MiddleInitial'
            section='Employee Information' if pi==0 and w.rect.y0<380 else 'Employer Review and Verification' if pi==0 else 'Preparer or Translator' if w.rect.y0>150 else 'Employee Information from Section 1'
            annotations.append(dict(source='i9',page=pi+1,field=name,klass=klass,label=clean(w.field_label or name),
                section=section,rect=list(w.rect),native_name=name,tooltip=w.field_label or '',origin='native PDF text widget'))
    # Both gold and extracted tracks use geometry order, independent of
    # annotation-author loop order and expected classes. Preserve ID mapping.
    source_order={source['id']:i for i,source in enumerate(manifest)}
    annotations.sort(key=lambda f:(source_order[f['source']],f['page'],round(f['rect'][1]/5),f['rect'][0]))
    # Native PDFs sometimes repeat /T names; widget xrefs disambiguate slots.
    seen=set()
    for f in annotations:
        key=(f['source'],f['field'])
        if key in seen: f['field']+=f'@p{f["page"]}-'+str(len(seen))
        seen.add((f['source'],f['field']))
    split={s['id']:s['split'] for s in manifest}
    gold=['# split\tform\tfield\tclass\tlabel\tsection'];extracted=list(gold)
    for f in annotations:
        label,section,evidence=extract(docs[f['source']][f['page']-1],f)
        f.update(extracted_label=label,extracted_section=section,label_evidence=evidence)
        prefix=[split[f['source']],f['source'],f['field'],f['klass']]
        gold.append('\t'.join(prefix+[clean(f['label']),clean(f['section'])]))
        extracted.append('\t'.join(prefix+[label,section]))
    (DATA/'applications-gold.tsv').write_text('\n'.join(gold)+'\n', encoding='utf-8')
    (DATA/'applications-extracted.tsv').write_text('\n'.join(extracted)+'\n', encoding='utf-8')
    (DATA/'applications-fields.json').write_text(json.dumps(annotations,indent=2,ensure_ascii=False)+'\n', encoding='utf-8')
    for s in manifest:
        path=APP/(s['id']+'.pdf')
        s.update(sha256=hashlib.sha256(path.read_bytes()).hexdigest(),bytes=path.stat().st_size,
                 annotated_fields=sum(f['source']==s['id'] for f in annotations))
    (DATA/'applications.json').write_text(json.dumps(manifest,indent=2)+'\n', encoding='utf-8')
    print('Annotated',len(annotations),'slots across',len(manifest),'application families')



def fixtures(out):
    """Save annotated derivatives; benchmark extraction always uses originals."""
    from fieldml_corpus import expected, profile
    manifest=json.loads((DATA/'applications.json').read_text(encoding='utf-8'))
    annotations=json.loads((DATA/'applications-fields.json').read_text(encoding='utf-8'))
    groups=defaultdict(list)
    for f in annotations:groups[f['source']].append(f)
    prediction_rows=list(csv.DictReader((out/'predictions.tsv').open(encoding='utf-8'),delimiter='\t'))
    predictions={(r['form'],r['field']):r for r in prediction_rows if r['variant']=='contextual' and r['seed']=='1' and r['track'] in ['gold','synthetic']}
    dest=out/'pdfs';dest.mkdir(exist_ok=True)
    all_examples=[]
    def write_versions(source,fields,split,synthetic=False):
        p=profile(len(all_examples))
        normalized=[dict(id=f['field'],label=f['label'],klass=f['klass']) for f in fields]
        wants=[expected(f['klass'],f['label'],normalized,i,p) for i,f in enumerate(fields)]
        says=[expected(predictions[(source,f['field'])]['predicted'],f['label'],normalized,i,p) for i,f in enumerate(fields)]
        allowed=[r if predictions[(source,f['field'])]['accepted']=='true' else '' for f,r in zip(fields,says)]
        if synthetic:
            doc=fitz.open();page=None;y=800;previous=None
            for f in fields:
                if y>720:
                    page=doc.new_page();page.insert_text((24,32),source,fontsize=9);y=60;previous=None
                if f['section']!=previous:
                    heading=f['section'] or 'Additional fields'
                    page.insert_text((24,y),heading,fontsize=10);y+=18;previous=f['section']
                page.insert_text((24,y),f['label'][:60],fontsize=8)
                f['page']=page.number+1;f['rect']=[305,y-10,570,y+5];page.draw_rect(fitz.Rect(f['rect']),color=(.6,.6,.6),width=.5);y+=25
            base=doc.tobytes();doc.close()
        else:base=(APP/(source+'.pdf')).read_bytes()
        expected_map={f['field']:v for f,v in zip(fields,wants)}
        for version,values in [('blank',['']*len(fields)),('expected',wants),('predicted',says),('accepted',allowed)]:
            doc=fitz.open(stream=base,filetype='pdf')
            for f,value in zip(fields,values):
                page=doc[f['page']-1]
                widget=None
                if f.get('native_name'):
                    widget=next((w for w in page.widgets() or [] if w.field_name==f['native_name'] and abs(w.rect.x0-f['rect'][0])<2 and abs(w.rect.y0-f['rect'][1])<2),None)
                    if widget is None:raise ValueError(('missing source widget',source,f['field']))
                if widget is None:
                    widget=fitz.Widget();widget.field_type=fitz.PDF_WIDGET_TYPE_TEXT
                    widget.field_name='fixture-'+f['field'];widget.rect=fitz.Rect(f['rect'])
                    # Gold-derived metadata is fixture-only. Never feed these
                    # derivative tooltips back into the extracted benchmark.
                    widget.field_label=f['label'];widget.text_fontsize=8;widget.text_color=(0,0,.6)
                    widget.field_value=value;page.add_widget(widget)
                else:
                    widget.field_value=value;widget.text_fontsize=8;widget.text_color=(0,0,.6);widget.update()
            for page in doc:
                page.insert_text((24,12),'FICTIONAL TRAINING EXAMPLE - NOT FOR SUBMISSION',fontsize=7,color=(.8,0,0))
            path=dest/f'{source}-{version}.pdf';doc.save(path,garbage=3,deflate=True);doc.close()
            check=fitz.open(path)
            for f,value in zip(fields,values):
                candidates=[w for w in check[f['page']-1].widgets() or [] if abs(w.rect.x0-f['rect'][0])<2 and abs(w.rect.y0-f['rect'][1])<2]
                assert any((w.field_value or '')==value for w in candidates),(path,f['field'],value)
            # Preserve every original printed word (field appearances excluded).
            original=fitz.open(stream=base,filetype='pdf')
            for before,after in zip(original,check):
                # Original page content streams remain embedded; changed widget
                # appearances are deliberately different, so don't compare all text.
                for stream in before.get_contents():
                    assert original.xref_stream(stream) in b'\n'.join(check.xref_stream(s) for s in after.get_contents())
            original.close();check.close()
        records=[dict(**f,expected_value=value,predicted_class=predictions[(source,f['field'])]['predicted'],predicted_value=said,
                      accepted=predictions[(source,f['field'])]['accepted']=='true',accepted_value=accepted,
                      correction_needed=predictions[(source,f['field'])]['predicted']!=f['klass'])
                 for f,value,said,accepted in zip(fields,wants,says,allowed)]
        record=dict(form=source,split=split,profile=p,track='gold labels and sections',
                    derivative_note='Manual printable slots become annotated widgets only in these fixtures; benchmark uses source originals.',fields=records)
        (dest/f'{source}.json').write_text(json.dumps(record,indent=2)+'\n', encoding='utf-8');all_examples.append(record)
    for m in manifest:write_versions(m['id'],groups[m['id']],m['split'])
    synthetic=defaultdict(list);splits={}
    for row in csv.reader((out/'synthetic.tsv').read_text(encoding='utf-8').splitlines(),delimiter='\t'):
        if not row or row[0].startswith('#'):continue
        split,source,field,klass,label,section=row
        if int(source.rsplit('-',1)[-1]) >= {'train':8,'validation':4,'test':4}[split]:continue
        synthetic[source].append(dict(source=source,field=field,klass=klass,label=label,section=section));splits[source]=split
    for source,fields in synthetic.items():write_versions(source,fields,splits[source],True)
    (out/'expectations.jsonl').write_text(''.join(json.dumps(r)+'\n' for r in all_examples), encoding='utf-8')
    print('Saved and read back',len(all_examples),'blank/expected/predicted/accepted PDF sets')

if __name__=='__main__':
    import argparse
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--fixtures',type=Path)
    args=parser.parse_args()
    if args.fixtures:fixtures(args.fixtures)
    else:build()
