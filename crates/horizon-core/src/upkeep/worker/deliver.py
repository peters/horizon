"""Writes Horizon's reports of native tests for the Upkeep agent. Horizon sends this file ahead
of exchange.py, whose checked folder and file helpers it uses, and runs it the same way: as the
account that owns /workspace/home."""
import json
import os
import re

REPORT_NAME = re.compile(r'[A-Za-z0-9-]{1,39}__[A-Za-z0-9._-]{1,100}__[0-9]{1,8}\.json')


def deliver(reports):
    """Writes Horizon's reports of native tests to native/results, one file per pull request,
    and removes the reports Horizon no longer sends."""
    home = owned_folder(HOME)
    try:
        upkeep = folder(home, FOLDER)
        try:
            base = folder(upkeep, NATIVE)
            try:
                results = folder(base, 'results')
                try:
                    wanted = set()
                    for report in reports:
                        if not isinstance(report, dict):
                            continue
                        name = f"{str(report.get('repository', '')).replace('/', '__', 1)}__{report.get('pull_request')}.json"
                        if REPORT_NAME.fullmatch(name):
                            wanted.add(name)
                            try:
                                write(results, name, json.dumps(report, indent=1))
                            except Refused:
                                # A folder the agent made under this name keeps only this report out.
                                continue
                    for name in os.listdir(results):
                        if name.endswith('.json') and not name.startswith('.') and name not in wanted:
                            try:
                                remove(results, name)
                            except Refused:
                                continue
                finally:
                    os.close(results)
            finally:
                os.close(base)
        finally:
            os.close(upkeep)
    finally:
        os.close(home)
    return json.dumps({'delivered': len(reports)})
