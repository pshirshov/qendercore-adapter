import fs from 'fs';
import path from 'path';

const API_URL = 'https://auth.qendercore.com:8000/v1';
const CACHE_DIR = '.cache';
const TOKEN_FILE = path.join(CACHE_DIR, 'token.json');

export const ScheduleMode = {
    DISABLE: 0,
    FORCED_CHARGE: 1,
    FORCED_DISCHARGE: 2,
};

const SCHEDULE_MODE_NAMES = {
    [ScheduleMode.DISABLE]: 'Disable',
    [ScheduleMode.FORCED_CHARGE]: 'Forced Charge',
    [ScheduleMode.FORCED_DISCHARGE]: 'Forced Discharge',
};

export class Schedule {
    constructor(mode, startTime, endTime) {
        this.mode = mode;
        this.startTime = startTime;
        this.endTime = endTime;
    }

    toString() {
        if (this.mode === ScheduleMode.DISABLE) {
            return 'Disabled';
        }
        const modeName = SCHEDULE_MODE_NAMES[this.mode];
        return `${modeName}: ${this.startTime} - ${this.endTime}`;
    }
}

export class DailyScheduleConfig {
    constructor(stateEnabled, forceChargeCurrent, forceDischargeCurrent, minSoc, maxSoc, schedules) {
        this.stateEnabled = stateEnabled;
        this.forceChargeCurrent = forceChargeCurrent;
        this.forceDischargeCurrent = forceDischargeCurrent;
        this.minSoc = minSoc;
        this.maxSoc = maxSoc;
        this.schedules = schedules;
    }

    toString() {
        const lines = [
            `Schedule State: ${this.stateEnabled ? 'On' : 'Off'}`,
            `Force Charge Current: ${this.forceChargeCurrent} A`,
            `Force Discharge Current: ${this.forceDischargeCurrent} A`,
            `Min SoC: ${this.minSoc}%`,
            `Max SoC: ${this.maxSoc}%`,
            'Schedules:',
        ];
        this.schedules.forEach((sched, i) => {
            lines.push(`  ${i + 1}. ${sched.toString()}`);
        });
        return lines.join('\n');
    }
}

function getHeaders() {
    return {
        'User-Agent': 'Mozilla/5.0 (X11; Linux x86_64; rv:124.0) Gecko/20100101 Firefox/124.0',
        'Origin': 'https://www.qendercore.com',
        'Referer': 'https://www.qendercore.com',
        'Accept': 'application/json',
        'Accept-Encoding': 'gzip, deflate, br',
        'Accept-Language': 'en-US,en;q=0.5',
        'Cache-Control': 'no-cache',
        'Pragma': 'no-cache',
        'Connection': 'keep-alive',
        'Sec-Fetch-Dest': 'empty',
        'Sec-Fetch-Mode': 'cors',
        'Sec-Fetch-Site': 'same-site',
        'Sec-GPC': '1',
        'x-qc-client-seq': 'W.1.1',
    };
}

async function getToken(login, password) {
    const response = await fetch(`${API_URL}/auth/login`, {
        method: 'POST',
        headers: {
            ...getHeaders(),
            'Content-Type': 'application/x-www-form-urlencoded',
        },
        body: new URLSearchParams({
            username: login,
            password: password,
        }),
    });

    const data = await response.json();
    return data.access_token;
}

async function validateToken(token) {
    try {
        const response = await fetch(`${API_URL}/s/accountinfo`, {
            method: 'GET',
            headers: {
                ...getHeaders(),
                'Authorization': `Bearer ${token}`,
            },
        });

        const data = await response.json();
        if (data.uid) {
            return true;
        }
        console.error('Error: Unexpected validation response');
        return false;
    } catch (e) {
        console.error(`Error: Failed to validate token: ${e.message}`);
        return false;
    }
}

async function getCachedToken(login, password) {
    try {
        const data = JSON.parse(fs.readFileSync(TOKEN_FILE, 'utf-8'));
        const token = data.token;
        const isValid = await validateToken(token);
        if (isValid) {
            return token;
        }
        throw new Error('Invalid token');
    } catch {
        const token = await getToken(login, password);
        fs.mkdirSync(CACHE_DIR, { recursive: true });
        fs.writeFileSync(TOKEN_FILE, JSON.stringify({ token }));
        return token;
    }
}

async function getDeviceId(token) {
    const response = await fetch(`${API_URL}/s/dashboard`, {
        method: 'GET',
        headers: {
            ...getHeaders(),
            'Authorization': `Bearer ${token}`,
        },
    });

    const dashboard = await response.json();

    if (!dashboard.rows) throw new Error("Dashboard response missing 'rows'");
    if (dashboard.rows.length === 0) throw new Error('Dashboard has no rows');
    if (dashboard.rows[0].cells.length === 0) throw new Error('Dashboard row has no cells');

    const firstWidget = dashboard.rows[0].cells[0].widget;
    const deviceId = firstWidget.datafetch.parameters.deviceId;

    if (!deviceId) throw new Error('Device ID not found in dashboard');
    return deviceId;
}

function normalizeKey(name) {
    return name.toLowerCase().replace(/ /g, '_').replace(/[().-]/g, '_').replace(/_+/g, '_').replace(/^_|_$/g, '');
}

function flatten(arr) {
    return arr.reduce((acc, val) => acc.concat(val), []);
}

export async function fetchInverterData(login, password) {
    const token = await getCachedToken(login, password);

    const dashboardResponse = await fetch(`${API_URL}/s/dashboard`, {
        method: 'GET',
        headers: {
            ...getHeaders(),
            'Authorization': `Bearer ${token}`,
        },
    });

    const dashboard = await dashboardResponse.json();
    const rows = flatten(dashboard.rows.map(r => r.cells));
    const devparams = rows.map(w => w.widget);

    const idtoparams = devparams.map(p => ({
        datafetch: {
            fetchType: p.datafetch.fetchType,
            deviceId: p.datafetch.parameters.deviceId,
            ...p.datafetch.parameters,
        },
        echartOpts: p.echartOpts,
    }));
    const titles = devparams.map(w => w.title);

    const result = {
        status: {},
        energy: {},
        powerHistory: [],
    };

    for (let idx = 0; idx < idtoparams.length; idx++) {
        const param = idtoparams[idx];

        const chartResponse = await fetch(`${API_URL}/h/chart`, {
            method: 'POST',
            headers: {
                ...getHeaders(),
                'Authorization': `Bearer ${token}`,
                'Content-Type': 'application/json',
            },
            body: JSON.stringify(param),
        });

        const chartData = await chartResponse.json();
        const series = chartData.series;

        if (series.links) {
            for (const link of series.links) {
                const key = normalizeKey(link.id) + '_wh';
                result.status[key] = link.value;
            }
        } else if (chartData.dataset) {
            const legend = ['timestamp', ...series.map(e => normalizeKey(e.name))];
            const points = chartData.dataset.source;
            result.powerHistory = points.map(p => {
                const obj = {};
                legend.forEach((key, i) => {
                    obj[key] = p[i];
                });
                return obj;
            });
        } else if (Array.isArray(series)) {
            for (const element of series) {
                if (element.data) {
                    for (const d of element.data) {
                        const key = d.name ? normalizeKey(d.name) : normalizeKey(titles[idx]);
                        result.energy[key] = d.value;
                    }
                }
            }
        }
    }

    return result;
}

export async function fetchDailySchedule(login, password, deviceId = null) {
    const token = await getCachedToken(login, password);

    if (!deviceId) {
        deviceId = await getDeviceId(token);
    }

    const response = await fetch(`${API_URL}/h/devices/${deviceId}/widgets/dailysched`, {
        method: 'GET',
        headers: {
            ...getHeaders(),
            'Authorization': `Bearer ${token}`,
        },
    });

    const resp = await response.json();
    if (!resp.filters) throw new Error("Schedule response missing 'filters'");

    const values = {};
    for (const filter of resp.filters) {
        if (filter.output && filter.init !== undefined) {
            values[filter.output] = filter.init;
        }
    }

    const schedules = [];
    for (let i = 1; i <= 5; i++) {
        const modeKey = `s${i}_mode`;
        const startKey = `s${i}_starttime`;
        const endKey = `s${i}_endtime`;

        if (!values[modeKey]) throw new Error(`Missing ${modeKey} in schedule response`);
        if (!values[startKey]) throw new Error(`Missing ${startKey} in schedule response`);
        if (!values[endKey]) throw new Error(`Missing ${endKey} in schedule response`);

        schedules.push(new Schedule(
            parseInt(values[modeKey]),
            values[startKey],
            values[endKey]
        ));
    }

    return new DailyScheduleConfig(
        values.sched_state === '1',
        parseFloat(values.force_charge_curr),
        parseFloat(values.force_discharge_curr),
        parseInt(values.min_soc),
        parseInt(values.max_soc),
        schedules
    );
}

export async function setDailySchedule(login, password, config, deviceId = null) {
    const token = await getCachedToken(login, password);

    if (!deviceId) {
        deviceId = await getDeviceId(token);
    }

    const body = {
        sched_state: config.stateEnabled ? '1' : '0',
        force_charge_curr: String(config.forceChargeCurrent),
        force_discharge_curr: String(config.forceDischargeCurrent),
        min_soc: String(config.minSoc),
        max_soc: String(config.maxSoc),
    };

    config.schedules.forEach((sched, i) => {
        body[`s${i + 1}_mode`] = String(sched.mode);
        body[`s${i + 1}_starttime`] = sched.startTime;
        body[`s${i + 1}_endtime`] = sched.endTime;
    });

    for (let i = config.schedules.length + 1; i <= 5; i++) {
        body[`s${i}_mode`] = '0';
        body[`s${i}_starttime`] = '00:00:00';
        body[`s${i}_endtime`] = '00:00:00';
    }

    const response = await fetch(`${API_URL}/h/devices/${deviceId}/solt/daysched`, {
        method: 'POST',
        headers: {
            ...getHeaders(),
            'Authorization': `Bearer ${token}`,
            'Content-Type': 'application/json',
        },
        body: JSON.stringify(body),
    });

    return await response.json();
}
