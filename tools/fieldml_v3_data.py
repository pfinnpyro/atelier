#!/usr/bin/env python3
"""Rebuild v3 development annotations without mutating archived v2 inputs."""
from pathlib import Path
import json
import fieldml_applications as previous
from fieldml_labels import extract, label_rects

DATA=Path(__file__).resolve().parents[1]/'crates/atelier-fieldml/data'


def build():
    target=DATA/'v3/annotated';target.mkdir(parents=True,exist_ok=True)
    (target/'applications.json').write_bytes((DATA/'applications.json').read_bytes())
    previous.DATA=target
    previous.extract=extract
    previous.label_rects=label_rects
    previous.build()
    manifest=json.loads((target/'applications.json').read_text())
    splits={s['id']:('validation' if s['split']=='validation' else 'train') for s in manifest}
    for kind in ['gold','extracted']:
        rows=(target/f'applications-{kind}.tsv').read_text().splitlines()
        output=['# V2 tests retired into development training; validation families stay disjoint.']
        for row in rows:
            if row.startswith('#'):continue
            fields=row.split('\t');fields[0]=splits[fields[1]];output.append('\t'.join(fields))
        (DATA/f'v3/development-{kind}.tsv').write_text('\n'.join(output)+'\n')
    (DATA/'v3/development-fields.json').write_bytes((target/'applications-fields.json').read_bytes())

if __name__=='__main__':build()
