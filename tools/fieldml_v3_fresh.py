#!/usr/bin/env python3
"""Reviewed fresh-family slots. Extraction never reads the expected labels/classes."""
import json,csv,hashlib
from pathlib import Path
import pymupdf as fitz
from fieldml_labels import label_rects,extract,clean
DATA=Path(__file__).resolve().parents[1]/'crates/atelier-fieldml/data/v3'


def build():
    candidates=json.loads((DATA/'fresh-sources.json').read_text(encoding='utf-8'))
    sources=[s for s in candidates if 'sha256' in s]
    docs={s['id']:fitz.open(DATA/'sources'/(s['id']+'.pdf')) for s in sources}
    fields=[]
    def manual(id,page,label,klass='Other',section='',occ=0,rect=None,placement='right',gold=None):
        p=docs[id][page-1];hits=label_rects(p,label)
        if id=='patient-ecw' and section.startswith('RESPONSIBLE'):
            hits=[h for h in hits if 300<h.y0<420];occ=0
        if id=='patient-ecw' and section=='EMERGENCY CONTACT INFORMATION':
            hits=[h for h in hits if 420<h.y0<535];occ=0
        if occ>=len(hits):raise ValueError((id,page,label,occ,len(hits)))
        b=hits[occ]
        if rect is None:
            if placement=='above':rect=[b.x0,b.y0-19,b.x0+85,b.y0-2]
            elif placement=='below':rect=[b.x0,b.y1+2,min(p.rect.width-18,b.x0+160),b.y1+17]
            else:rect=[b.x1+2,b.y0,min(p.rect.width-18,b.x1+82),b.y1]
        fields.append(dict(source=id,page=page,field=f'p{page}-{len(fields):03}',label=gold or label,klass=klass,section=section,rect=rect,label_rect=list(b),native_name='',tooltip='',origin='reviewed printable blank slot'))
    # Calibration: primary applicant and every selected alternate/contact group.
    for label,klass in [('SURNAME:','LastName'),('FIRST NAME:','FirstName'),('Marital Status:','Other'),('Date of Birth:','BirthDate'),('Place of Birth:','Other'),('Nationality:','Other'),('Residential address:','AddressLine1'),('Parish/City:','City'),('State/Country:','Other'),('Zip/Postal Code:','Zip'),('Home #:','Phone'),('Work #:','Other'),('Cell #:','Phone'),('Email','Email')]:
        manual('membership-rhadc',1,label,klass,'PRIMARY APPLICANT INFORMATION',placement='below' if label=='Email' else 'right')
    for label in ['Mailing Address (if different from above):','Parish/City:','State/Country:','Zip/Postal Code:']:
        manual('membership-rhadc',1,label,'Other','Mailing Address (if different)',occ=0 if label.startswith('Mailing') else 1)
    for label in ['Current employer:','Occupation:','Business Tel:','Business Email:']:
        manual('membership-rhadc',2,label,'Other','EMPLOYMENT INFORMATION')
    for label in ['Name:','Date of Birth:','Cell #:','Email:']:
        manual('membership-rhadc',2,label,'Other','ASSOCIATE APPLICANT (SPOUSE/PARTNER)')
    for i in range(3):
        manual('membership-rhadc',2,f'Name (Dependent {i+1}):','Other','DEPENDANT INFORMATION')
        manual('membership-rhadc',2,'Date of Birth:','Other','DEPENDANT INFORMATION',occ=i+1)
    for label in ['Proposer:','Seconder:','Credit Card Number','Expiration Date','Hobbies:']:
        manual('membership-rhadc',2,label)
    for i in range(5):manual('membership-rhadc',3,'Date:','Other','Declaration',occ=i)
    for label in ['Signature of Applicant:','Signature of Spouse/Partner:']:
        manual('membership-rhadc',3,label,'Other','Declaration')
    # Employment: the name/address sublabels are printed BELOW the writable row.
    for label,klass,rect in [
        ('Last','LastName',[115,236,230,253]),('First','FirstName',[235,236,325,253]),('M.I.','MiddleInitial',[328,236,385,253]),
        ('Street address','AddressLine1',[115,274,320,290]),('Apt/Unit #','AddressLine2',[325,274,385,290]),
        ('City','City',[115,311,270,328]),('State','State',[275,311,325,328]),('Zip Code','Zip',[328,311,385,328])]:
        manual('employment-humboldt',1,label,klass,'Application information',rect=rect)
    for label,klass in [('Phone:','Phone'),('Email:','Email'),('Date:','Other'),('S.S. no:','Other'),('Desired salary:','Other'),('Restrictions','Other')]:
        manual('employment-humboldt',1,label,klass,'Application information')
    for page in [2,3]:
        p=docs['employment-humboldt'][page-1]
        # The selected pages contain only emergency, education, references,
        # prior employment and military fields. Every readable writing label is negative.
        for block in p.get_text('dict')['blocks']:
            for line in block.get('lines',[]):
                label=''.join(s['text'] for s in line['spans']).strip()
                if (not label.endswith(':') and label.lower()!='company') or len(label)>70 or not any(c.isalnum() for c in label):continue
                b=fitz.Rect(line['bbox'])
                context='Emergency Contact' if page==2 and b.y0<130 else 'Education' if page==2 and b.y0<370 else 'References' if page==2 else 'Previous Employment'
                manual('employment-humboldt',page,label,'Other',context,rect=[b.x1+2,b.y0,min(p.rect.width-18,b.x1+80),b.y1],occ=sum(1 for f in fields if f['source']=='employment-humboldt' and f['page']==page and f['label']==label))
    for label in ['Signature:','Date:']:manual('employment-humboldt',4,label)
    # eCW: all demographic text fields, responsible-party and emergency groups.
    for label,klass in [("Patient’s Legal Name: (Last)",'LastName'),('(First)','FirstName'),('(MI)','MiddleInitial'),
        ('Preferred Full Name (if different from above):','Other'),('Address:','AddressLine1'),('City, State, Zip:','CityStateZip'),
        ('Home Phone Number (landline):','Phone'),('Cell:','Phone'),('Work:','Other'),('E-Mail Address:','Email'),('Date of Birth:','BirthDate'),('Patient Social Security Number:','Other')]:
        manual('patient-ecw',1,label,klass,'PATIENT INFORMATION')
    for label,occ in [('Responsible party name: (Last)',0),('(First)',1),('(MI)',1),('Date of birth:',1),('Responsible Party Social Security Number:',0),('Phone number:',0),('Address:',1),('City, State:',0),('ZIP:',0)]:
        manual('patient-ecw',1,label,'Other','RESPONSIBLE PARTY INFORMATION (If not self)',occ=occ)
    for label,occ in [('Emergency contact name: (Last)',0),('(First)',2),('Phone number:',1),('Emergency contact relationship to patient:',0),('Address',2),('City, State:',1),('ZIP:',1),('Home phone:',0),('Work hone:',0),('Ext.',0)]:
        manual('patient-ecw',1,label,'Other','EMERGENCY CONTACT INFORMATION',occ=occ)
    for label in ['Signature of patient or personal representative:','Date:','Printed name of patient or personal representative:','Relationship to patient:']:
        manual('patient-ecw',1,label,'Other','GENERAL CONSENT')
    # Test: all native text widgets in the Xavier application (not checkboxes/signatures).
    positive={'Family Name':('LastName','Family Name'),'First Name':('FirstName','First Name'),
        'Bate of Birth':('BirthDate','Date of Birth (DD/MM/YYYY)'),
        'Postal Address':('AddressLine1','Postal Address'),'Postal Address Suburb':('City','Suburb'),
        'Postal Address Postcode':('Zip','Postcode'),'Contact Telephone Home':('Phone','Telephone Numbers (H)'),
        'Contact Telephone Mobile':('Phone','Telephone Numbers (M)'),'Email Address':('Email','Email Address')}
    for pi,p in enumerate(docs['volunteer-xavier']):
        for w in p.widgets() or []:
            if w.field_type!=fitz.PDF_WIDGET_TYPE_TEXT:continue
            klass,label=positive.get(w.field_name,('Other',w.field_label or w.field_name))
            ctx='Contact Details' if pi==0 and w.rect.y0<400 else 'Emergency Contact' if pi==0 and w.rect.y0<640 else 'Personal Information' if pi==0 else 'Volunteer Declaration'
            fields.append(dict(source='volunteer-xavier',page=pi+1,field=w.field_name,label=label,klass=klass,section=ctx,rect=list(w.rect),native_name=w.field_name,tooltip=w.field_label or '',origin='native text widget'))
    # Test: first camper, every adult/contact field and health narrative blank.
    for label,klass in [('Child’s Full Name:','FullName'),('Date of Birth (MM/DD/YYYY):','BirthDate')]:manual('camp-vbac',1,label,klass)
    for label in ['Parent/Guardian 1 Name:','Parent/Guardian 2 Name:']:
        manual('camp-vbac',1,label,'Other','Parent/Guardian Information')
    for label in ['Relationship to Camper:','Home:','Cell:','Email Address:']:
        for i in range(2):manual('camp-vbac',1,label,'Other','Parent/Guardian Information',occ=i)
    for label,occ in [('Emergency Contact Name:',0),('Relationship to Camper:',2),('Phone Number:',2)]:manual('camp-vbac',1,label,'Other','Emergency Contact Information',occ=occ)
    for label in ['Name:','Relationship:']:
        for i in range(2):manual('camp-vbac',1,label,'Other','Authorized Individuals for Drop Off/Pick Up',occ=i+3 if label=='Name:' else i)
    # Last two Phone Number labels are the authorized pickup people.
    for i in [3,4]:manual('camp-vbac',1,'Phone Number:','Other','Authorized Individuals for Drop Off/Pick Up',occ=i)
    for i in range(3):manual('camp-vbac',2,'Yes, please specify:','Other','Health Information',occ=i)
    for label in ['Yes, please explain:','Signature:','Date:']:manual('camp-vbac',2,label,'Other','Consent and Agreements')
    # Test rental: all personal/current, past residence, employer and reference text fields.
    for label,klass in [('APPLICATION DATE:','Other'),('For Property at','Other'),('Projected move-in date (if accepted)','Other'),('Rental Amount','Other'),('Title','Honorific'),('Full Given Names','Other'),('Surname','LastName'),('Date of Birth','BirthDate'),('Social Security Number','Other'),('Driver’s License #','Other'),('State','Other'),('Current Phone','Phone'),('Email','Email')]:
        manual('rental-dixon',1,label,klass,'Driver’s License' if label=='State' else 'PERSONAL INFORMATION')
    for label,klass,occ in [('Street','AddressLine1',0),('City','City',0),('State','State',1),('Zip','Zip',0),('Current Landlord’s Name','Other',0),('Phone','Other',1),('Street','Other',1),('City','Other',1),('State','Other',2),('Zip','Other',1),('Current Landlord’s Name','Other',1),('Phone','Other',2)]:
        manual('rental-dixon',1,label,klass,'PRESENT ADDRESS' if (klass!='Other' or (label in ['Current Landlord’s Name','Phone'] and occ in [0,1])) else 'PREVIOUS ADDRESS',occ=occ)
    for label in ['List Names of Additional Occupants','How many pets?','Breed, weight, size?','Applicant Occupation','Current Employer','Position','Address','How long employed? (Years/Months)','Gross Income','Supervisor/HR Contact','Phone','Previous Employer','Why did you leave?','Bank Name','Checking Acct #','Savings Acct #','Name','Relationship to Applicant']:
        # Include each printed occurrence on this all-negative page.
        for i in range(len(label_rects(docs['rental-dixon'][1],label))):manual('rental-dixon',2,label,'Other','PERSONAL REFERENCES' if label in ['Name','Relationship to Applicant'] else 'EMPLOYMENT INFORMATION',occ=i)
    for label in ['Name','City','State','Zip','Relationship to Applicant','Phone']:
        manual('rental-dixon',3,label,'Other','EMERGENCY CONTACT')
    # Test Japanese Association: English-labeled applicant/company/family slots.
    # Include implicit postcode boxes: their printed label is Singapore, so
    # successful extraction/classification must infer the postal role from context.
    # Actual writing lines sit BELOW the bilingual labels, not at English text endpoints.
    jas=[
        ('Applicant’s Surname','LastName','Particulars of Applicant',[313,61,451,73]),
        ('First Name or / and Christian Name','FirstName','Particulars of Applicant',[230,90,451,101]),
        ('Chinese Characters (if any)','Other','Particulars of Applicant',[220,118,360,130]),
        ('Passport / IC No.','Other','Particulars of Applicant',[390,146,531,158]),
        ('Nationality','Other','Particulars of Applicant',[143,174,220,186]),
        ('Date of Birth:','BirthDate','Particulars of Applicant',[165,200,272,215]),
        ('Place of Birth','Other','Particulars of Applicant',[354,207,531,218]),
        ('Age','Other','Particulars of Applicant',[125,230,220,243]),
        ('Designation','Other','Particulars of Applicant',[312,230,531,243]),
        ('Name of Company','Other','Name of Company',[153,258,531,272]),
        ('Address of Company','Other','Address of Company',[153,285,531,300]),
        ('Singapore','Other','Address of Company',[202,328,270,340]),
        ('Tel No.:','Other','Address of Company',[327,328,401,340]),
        ('Fax No.:','Other','Address of Company',[443,328,531,340]),
        ('Residential Address','AddressLine1','Residential Address',[180,361,531,376]),
        ('Singapore','Zip','Residential Address',[202,402,270,415]),
        ('Tel No.:','Phone','Residential Address',[327,402,401,415]),
        ('Fax No.:','Other','Residential Address',[443,402,531,415]),
        ('H/P No.','Phone','Residential Address',[247,429,531,441]),
        ('E-mail Address','Email','Residential Address',[205,525,531,539]),
        ('Special Skills','Other','Personal activities',[140,565,531,580]),
        ('Hobbies','Other','Personal activities',[140,593,531,608]),
        ('Qualifications/License','Other','Personal activities',[140,621,531,636]),
        ('Membership of Other Club','Other','Personal activities',[245,649,531,664]),
        ('Public Commendation','Other','Personal activities',[140,677,531,692]),
        ('Criminal Records','Other','Personal activities',[140,705,531,720]),
    ]
    for label,klass,context,rect in jas:
        occ=1 if label in ['Tel No.:','Fax No.:','Singapore'] and context=='Residential Address' else 0
        manual('membership-jas',2,label,klass,context,occ=occ,rect=rect)
    for label in ['Surname','First Name or / and Christian Name','Chinese Characters (if any)','Passport / IC No.','Occupation','H/P No.','Nationality','Date of Birth:','Special Skills','Hobbies','Qualifications/License']:
        manual('membership-jas',3,label,'Other','SPOUSE')
    # Four dependent columns; preserve their identical field names as distinct slots.
    for column in range(4):
        x=180+column*96
        for label,y in [('Full Name',445),('Chinese Characters',486),('Date of Birth',566),('Signature',606)]:
            manual('membership-jas',3,label,'Other','Applicant’s CHILDREN & PARENTS',occ=0,rect=[x,y,x+85,y+15])
    # Identical printed labels are not source IDs; all inference inputs come from original PDF evidence.
    order={s['id']:i for i,s in enumerate(sources)};split={s['id']:s['split'] for s in sources}
    fields.sort(key=lambda f:(order[f['source']],f['page'],round(f['rect'][1]/5),f['rect'][0]))
    gold=['# split\tform\tfield\tclass\tlabel\tsection'];actual=list(gold)
    for f in fields:
        label,context,evidence=extract(docs[f['source']][f['page']-1],f)
        f.update(extracted_label=label,extracted_section=context,label_evidence=evidence)
        stage='validation' if split[f['source']]=='calibration' else 'test'
        prefix=[stage,f['source'],f['field'],f['klass']]
        gold.append('\t'.join(prefix+[clean(f['label']),clean(f['section'])]));actual.append('\t'.join(prefix+[label,context]))
    for source in sources:
        path=DATA/'sources'/(source['id']+'.pdf')
        assert hashlib.sha256(path.read_bytes()).hexdigest()==source['sha256']
        source['annotated_fields']=sum(f['source']==source['id'] for f in fields)
        source['coverage']='Reviewed text slots in core applicant, conditional and third-party groups; checkbox choices and unsupported implicit labels excluded.'
    (DATA/'fresh-reviewed-sources.json').write_text(json.dumps(sources,indent=2)+'\n', encoding='utf-8')
    (DATA/'fresh-fields.json').write_text(json.dumps(fields,indent=2,ensure_ascii=False)+'\n', encoding='utf-8')
    (DATA/'fresh-gold.tsv').write_text('\n'.join(gold)+'\n', encoding='utf-8');(DATA/'fresh-extracted.tsv').write_text('\n'.join(actual)+'\n', encoding='utf-8')
    print('Fresh annotated slots:',len(fields),'families:',len(sources))

if __name__=='__main__':build()
