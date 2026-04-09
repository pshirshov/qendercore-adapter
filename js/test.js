import fs from 'fs';
import { fetchInverterData, fetchDailySchedule, setDailySchedule, ScheduleMode, Schedule } from './index.js';

async function main() {
    const credentialsFile = process.argv[2] || '/var/run/agenix/qendercore';
    const authData = JSON.parse(fs.readFileSync(credentialsFile, 'utf-8'));
    const login = authData.login;
    const password = authData.password;

    console.log('=== Inverter Data (JSON) ===\n');
    const data = await fetchInverterData(login, password);

    const inverterJson = {
        status: data.status,
        energy: data.energy,
        powerHistoryEntries: data.powerHistory.length,
    };
    console.log(JSON.stringify(inverterJson, null, 2));

    console.log('\n=== Daily Schedule (JSON) ===\n');
    const config = await fetchDailySchedule(login, password);

    const scheduleJson = {
        stateEnabled: config.stateEnabled,
        forceChargeCurrent: config.forceChargeCurrent,
        forceDischargeCurrent: config.forceDischargeCurrent,
        minSoc: config.minSoc,
        maxSoc: config.maxSoc,
        schedules: config.schedules.map((s, i) => ({
            slot: i + 1,
            mode: s.mode,
            modeName: s.mode === ScheduleMode.DISABLE ? 'Disable' :
                      s.mode === ScheduleMode.FORCED_CHARGE ? 'Forced Charge' : 'Forced Discharge',
            startTime: s.startTime,
            endTime: s.endTime,
        })),
    };
    console.log(JSON.stringify(scheduleJson, null, 2));

    console.log('\n=== Testing setDailySchedule (re-saving same config) ===\n');
    const response = await setDailySchedule(login, password, config);

    if (response.msgs && response.msgs.length > 0) {
        for (const msg of response.msgs) {
            console.log(`Response: ${msg.msg}`);
        }
    } else {
        console.log(`Response: ${JSON.stringify(response, null, 2)}`);
    }
}

main().catch(console.error);
