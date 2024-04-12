import json

import qcore

if __name__ == "__main__":
    with open('auth.json') as f:
        data = json.loads(f.read())
        login = data['login']
        pw = data['password']
        qcore.fetch_qc_data(login, pw)
