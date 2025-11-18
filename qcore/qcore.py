import json
import os
import sys
import urllib3
from dataclasses import dataclass
from typing import List

API_URL = 'https://auth.qendercore.com:8000/v1'
CACHE_DIR = '.cache'
TOKEN_FILE = os.path.join(CACHE_DIR, 'token.json')


class ScheduleMode:
    DISABLE = 0
    FORCED_CHARGE = 1
    FORCED_DISCHARGE = 2


SCHEDULE_MODE_NAMES = {
    ScheduleMode.DISABLE: "Disable",
    ScheduleMode.FORCED_CHARGE: "Forced Charge",
    ScheduleMode.FORCED_DISCHARGE: "Forced Discharge",
}


@dataclass
class Schedule:
    mode: int
    start_time: str
    end_time: str

    def __str__(self):
        if self.mode == ScheduleMode.DISABLE:
            return "Disabled"
        mode_name = SCHEDULE_MODE_NAMES[self.mode]
        return f"{mode_name}: {self.start_time} - {self.end_time}"


@dataclass
class DailyScheduleConfig:
    state_enabled: bool
    force_charge_current: float
    force_discharge_current: float
    min_soc: int
    max_soc: int
    schedules: List[Schedule]

    def __str__(self):
        lines = [
            f"Schedule State: {'On' if self.state_enabled else 'Off'}",
            f"Force Charge Current: {self.force_charge_current} A",
            f"Force Discharge Current: {self.force_discharge_current} A",
            f"Min SoC: {self.min_soc}%",
            f"Max SoC: {self.max_soc}%",
            "Schedules:"
        ]
        for i, sched in enumerate(self.schedules, 1):
            lines.append(f"  {i}. {sched}")
        return "\n".join(lines)


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
    token = resp_auth['access_token']
    return token


def get_cached_token(http, login, password):
    token = None
    try:
        with open(TOKEN_FILE) as f:
            data = json.loads(f.read())
            token = data['token']
            is_valid = validate_token(http, token)
            if is_valid:
                return token
            raise Exception("Invalid token")
    except Exception:
        token = get_token(http, login, password)
        os.makedirs(CACHE_DIR, exist_ok=True)
        with open(TOKEN_FILE, "w") as f:
            f.write(json.dumps({'token': token}))
    return token


def validate_token(http, token):
    try:
        req_account = http.request(
            'GET',
            '%s/s/accountinfo' % API_URL,
            headers={
                'Authorization': 'Bearer ' + token,
            }
        )
        resp_account = json.loads(req_account.data.decode('utf-8'))

        if "uid" in resp_account:
            return True
        else:
            print("Error: Unexpected validation response", file=sys.stderr)
            return False
    except Exception as e:
        print(f"Error: Failed to validate token: {e}", file=sys.stderr)
        return False


def flatten(xss):
    return [x for xs in xss for x in xs]


def fetch_qc_data(login, password):
    """
    Fetch current inverter data from the dashboard.

    Returns:
        dict with keys:
            - status: dict of current status values (e.g., grid_export, battery_discharge, battery_soc)
            - energy: dict of energy values (e.g., import_kwh, export_kwh, self_consumption_kwh)
            - power_history: list of dicts with timestamp and power values
    """
    http = get_http_client()
    token = get_cached_token(http, login, password)

    req_dashboard = http.request(
        'GET',
        '%s/s/dashboard' % API_URL,
        headers={
            'Authorization': 'Bearer ' + token,
        }
    )
    resp_dashboard = json.loads(req_dashboard.data.decode('utf-8'))
    rows = list(flatten(map(lambda r: r["cells"], resp_dashboard["rows"])))
    devparams = [w["widget"] for w in rows]

    idtoparams = list(
        map(lambda p: {
            'datafetch': {"fetchType": p["datafetch"]["fetchType"],
                          "deviceId": p["datafetch"]["parameters"]['deviceId']} | (
                             p["datafetch"]['parameters']),
            'echartOpts': p['echartOpts']},
            devparams))
    titles = [w["title"] for w in devparams]

    result = {
        'status': {},
        'energy': {},
        'power_history': []
    }

    for idx, param in enumerate(idtoparams):
        req_chart = http.request(
            'POST',
            '%s/h/chart' % API_URL,
            headers={
                'Authorization': 'Bearer ' + token,
            },
            body=json.dumps(param)
        )
        resp_chart = json.loads(req_chart.data.decode('utf-8'))
        series = resp_chart["series"]

        if "links" in series:
            # Status data (grid export, battery discharge, SoC)
            links = series["links"]
            for link in links:
                key = _normalize_key(link["id"])
                result['status'][key] = link["value"]
        elif "dataset" in resp_chart:
            # Time series power data
            legend = ["timestamp"] + [_normalize_key(e["name"]) for e in series]
            points = resp_chart["dataset"]["source"]
            result['power_history'] = [dict(zip(legend, p)) for p in points]
        elif type(series) is list:
            # Energy data
            for element in series:
                if "data" in element:
                    for d in element["data"]:
                        if "name" in d:
                            key = _normalize_key(d["name"])
                        else:
                            key = _normalize_key(titles[idx])
                        result['energy'][key] = d["value"]

    return result


def _normalize_key(name):
    """Convert display name to snake_case key"""
    return name.lower().replace(' ', '_').replace('(', '').replace(')', '').replace('.', '')


def get_http_client():
    """Create and return a configured HTTP client"""
    headers = {
        'User-Agent': 'Mozilla/5.0 (X11; Linux x86_64; rv:124.0) Gecko/20100101 Firefox/124.0',
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
    return urllib3.PoolManager(1, headers=headers)


def get_device_id(http, token):
    """Fetch the device ID from the dashboard"""
    req_dashboard = http.request(
        'GET',
        '%s/s/dashboard' % API_URL,
        headers={
            'Authorization': 'Bearer ' + token,
        }
    )
    resp_dashboard = json.loads(req_dashboard.data.decode('utf-8'))

    assert "rows" in resp_dashboard, "Dashboard response missing 'rows'"
    assert len(resp_dashboard["rows"]) > 0, "Dashboard has no rows"
    assert len(resp_dashboard["rows"][0]["cells"]) > 0, "Dashboard row has no cells"

    first_widget = resp_dashboard["rows"][0]["cells"][0]["widget"]
    device_id = first_widget["datafetch"]["parameters"]["deviceId"]

    assert device_id, "Device ID not found in dashboard"
    return device_id


def fetch_daily_schedule(login, password, device_id=None):
    """
    Fetch the current daily schedule configuration from the inverter.

    Args:
        login: Account email
        password: Account password
        device_id: Optional device ID. If not provided, will be fetched from dashboard.

    Returns:
        DailyScheduleConfig object with current schedule settings
    """
    http = get_http_client()
    token = get_cached_token(http, login, password)

    if device_id is None:
        device_id = get_device_id(http, token)

    req_schedule = http.request(
        'GET',
        '%s/h/devices/%s/widgets/dailysched' % (API_URL, device_id),
        headers={
            'Authorization': 'Bearer ' + token,
        }
    )

    resp = json.loads(req_schedule.data.decode('utf-8'))
    assert "filters" in resp, "Schedule response missing 'filters'"

    # Parse the response into a lookup dict
    values = {}
    for filter_elem in resp["filters"]:
        if "output" in filter_elem and "init" in filter_elem:
            values[filter_elem["output"]] = filter_elem["init"]

    # Extract schedules
    schedules = []
    for i in range(1, 6):
        mode_key = f"s{i}_mode"
        start_key = f"s{i}_starttime"
        end_key = f"s{i}_endtime"

        assert mode_key in values, f"Missing {mode_key} in schedule response"
        assert start_key in values, f"Missing {start_key} in schedule response"
        assert end_key in values, f"Missing {end_key} in schedule response"

        schedules.append(Schedule(
            mode=int(values[mode_key]),
            start_time=values[start_key],
            end_time=values[end_key]
        ))

    config = DailyScheduleConfig(
        state_enabled=values["sched_state"] == "1",
        force_charge_current=float(values["force_charge_curr"]),
        force_discharge_current=float(values["force_discharge_curr"]),
        min_soc=int(values["min_soc"]),
        max_soc=int(values["max_soc"]),
        schedules=schedules
    )

    return config


def set_daily_schedule(login, password, config, device_id=None):
    """
    Set the daily schedule configuration on the inverter.

    Args:
        login: Account email
        password: Account password
        config: DailyScheduleConfig object with desired settings
        device_id: Optional device ID. If not provided, will be fetched from dashboard.

    Returns:
        API response as dict
    """
    http = get_http_client()
    token = get_cached_token(http, login, password)

    if device_id is None:
        device_id = get_device_id(http, token)

    # Build the request body
    body = {
        "sched_state": "1" if config.state_enabled else "0",
        "force_charge_curr": str(config.force_charge_current),
        "force_discharge_curr": str(config.force_discharge_current),
        "min_soc": str(config.min_soc),
        "max_soc": str(config.max_soc),
    }

    # Add schedule slots
    for i, sched in enumerate(config.schedules, 1):
        body[f"s{i}_mode"] = str(sched.mode)
        body[f"s{i}_starttime"] = sched.start_time
        body[f"s{i}_endtime"] = sched.end_time

    # Fill remaining slots with disabled if less than 5 schedules provided
    for i in range(len(config.schedules) + 1, 6):
        body[f"s{i}_mode"] = "0"
        body[f"s{i}_starttime"] = "00:00:00"
        body[f"s{i}_endtime"] = "00:00:00"

    req = http.request(
        'POST',
        '%s/h/devices/%s/solt/daysched' % (API_URL, device_id),
        headers={
            'Authorization': 'Bearer ' + token,
            'Content-Type': 'application/json',
        },
        body=json.dumps(body)
    )

    resp = json.loads(req.data.decode('utf-8'))
    return resp
