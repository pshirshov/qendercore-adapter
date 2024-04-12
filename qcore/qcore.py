import json
import urllib3

API_URL = 'https://auth.qendercore.com:8000/v1'


def get_token(http, login, password):
    req_auth = http.request(
        'POST',
        '%s/auth/login' % API_URL,
        encode_multipart=False,
        fields={
            "username": login,
            "password": password
        },
    )

    resp_auth = json.loads(req_auth.data.decode('utf-8'))
    print(resp_auth)

    token = resp_auth['access_token']
    return token


def get_cached_token(http, login, password):
    token = None
    try:
        with open('token.json') as f:
            data = json.loads(f.read())
            token = data['token']
            isValid = validate_token(http, token)
            if isValid:
                return token
            raise Exception("Invalid token")
    except Exception as e:
        print("Failed to read token")
        print(e)
        token = get_token(http, login, password)
        with open('token.json', "w") as f:
            f.write(json.dumps({'token': token}))
    return token


def validate_token(http, token):
    try:
        req_account = http.request(
            'GET',
            '%s/v1/s/accountinfo' % API_URL,
            headers={
                'Authorization': 'Bearer ' + token,
            }
        )
        resp_account = json.loads(req_account.data.decode('utf-8'))
        with open('resp_account.json', "w") as f:
            f.write(json.dumps(resp_account, indent=2))
        return "uid" in resp_account
    except Exception as e:
        print("Failed to validate token")
        print(e)
        return False


def flatten(xss):
    return [x for xs in xss for x in xs]


def fetch_qc_data(login, password):
    headers = {'User-Agent': 'Mozilla/5.0 (X11; Linux x86_64; rv:124.0) Gecko/20100101 Firefox/124.0',
               'Origin': 'https://www.qendercore.com',
               'Referer': 'https://www.qendercore.com',
               'Accept': 'application/json',
               "Accept-Encoding": "gzip, deflate, br",
               "Accept-Language": "en-US,en;q=0.5",
               "Cache-Control": "no-cache",
               "Pragma": "no-cache",
               "Connection": "keep-alive",
               "Sec-Fetch-Dest": "empty",
               "Sec-Fetch-Mode": "cors",
               "Sec-Fetch-Site": "same-site",
               "Sec-GPC": 1,
               "x-qc-client-seq": "W.1.1",
               }
    http = urllib3.PoolManager(1, headers=headers)

    token = get_cached_token(http, login, password)

    ##
    req_dashboard = http.request(
        'GET',
        '%s/s/dashboard' % API_URL,
        headers={
            'Authorization': 'Bearer ' + token,
        }
    )
    resp_dashboard = json.loads(req_dashboard.data.decode('utf-8'))
    with open('resp_dashboard.json', "w") as f:
        f.write(json.dumps(resp_dashboard, indent=2))
    rows = list(flatten(map(lambda r: r["cells"], resp_dashboard["rows"])))
    devparams = [w["widget"] for w in rows]

    idtoparams = list(
        map(lambda p: {
            'datafetch': {"fetchType": p["datafetch"]["fetchType"],
                          "deviceId": p["datafetch"]["parameters"]['deviceId']} | p["datafetch"]['parameters'],
            'echartOpts': p['echartOpts']},
            devparams))
    titles = [w["title"] for w in devparams]

    ##
    for idx, param in enumerate(idtoparams):
        # print("chart # %s" % str(idx))
        req_dashboard_1 = http.request(
            'POST',
            '%s/h/chart' % API_URL,
            headers={
                'Authorization': 'Bearer ' + token,
            },
            body=json.dumps(param)
        )
        resp_dashboard_1 = json.loads(req_dashboard_1.data.decode('utf-8'))
        series = resp_dashboard_1["series"]

        if "links" in series:
            links = series["links"]
            for link in links:
                print("%s: %s" % (link["id"], link["value"]))
        elif "dataset" in resp_dashboard_1:
            legend = ["Timestamp"] + [e["name"] for e in series]
            points = resp_dashboard_1["dataset"]["source"]
            merged = [dict(zip(legend, p)) for p in points]
            print(merged)
        elif type(series) is list:
            for element in series:
                if "data" in element:
                    for d in element["data"]:
                        if "name" in d:
                            print("%s: %s" % (d["name"], d["value"]))
                        else:
                            print("%s: %s" % (titles[idx], d["value"]))

        with open('resp_chart_%d.json' % idx, "w") as f:
            f.write(json.dumps(resp_dashboard_1, indent=2))
