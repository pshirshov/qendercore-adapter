import logging

from ha_mqtt_discoverable import Settings, DeviceInfo
from ha_mqtt_discoverable.sensors import (
    Sensor, SensorInfo,
    Switch, SwitchInfo,
    Number, NumberInfo,
    Select, SelectInfo,
    Text, TextInfo,
)

from qcore import ScheduleMode, SCHEDULE_MODE_NAMES

logger = logging.getLogger(__name__)

DEVICE_INFO = DeviceInfo(
    name="Qendercore Inverter",
    identifiers="qendercore_inverter",
    manufacturer="Qendercore",
)

SCHEDULE_COUNT = 5

MODE_OPTIONS = [
    SCHEDULE_MODE_NAMES[ScheduleMode.DISABLE],
    SCHEDULE_MODE_NAMES[ScheduleMode.FORCED_CHARGE],
    SCHEDULE_MODE_NAMES[ScheduleMode.FORCED_DISCHARGE],
]

# fetch_qc_data() status dict keys -> (unique_id, display_name, device_class, unit)
STATUS_KEY_MAP = {
    "grid_export_wh": ("qc_grid_export", "Grid Export", "power", "W"),
    "battery_discharge_wh": ("qc_battery_discharge", "Battery Discharge", "power", "W"),
}

# fetch_qc_data() energy dict keys -> (unique_id, display_name, device_class, unit)
ENERGY_KEY_MAP = {
    "current_battery_soc": ("qc_battery_soc", "Battery SoC", "battery", "%"),
    "import_energy_kwh": ("qc_import_energy", "Import Energy", "energy", "kWh"),
    "export_energy_kwh": ("qc_export_energy", "Export Energy", "energy", "kWh"),
    "self_consumption_energy_kwh": ("qc_self_consumption", "Self Consumption", "energy", "kWh"),
}


def _make_sensor(mqtt_settings, unique_id, name, device_class, unit):
    info = SensorInfo(
        name=name,
        device_class=device_class,
        unique_id=unique_id,
        unit_of_measurement=unit,
        device=DEVICE_INFO,
    )
    return Sensor(Settings(mqtt=mqtt_settings, entity=info))


def create_sensors(mqtt_settings):
    """Create read-only sensor entities. Returns dict keyed by unique_id."""
    sensors = {}
    for _key, (uid, name, dc, unit) in STATUS_KEY_MAP.items():
        sensors[uid] = _make_sensor(mqtt_settings, uid, name, dc, unit)
    for _key, (uid, name, dc, unit) in ENERGY_KEY_MAP.items():
        sensors[uid] = _make_sensor(mqtt_settings, uid, name, dc, unit)
    sensors["qc_force_charge_current"] = _make_sensor(
        mqtt_settings, "qc_force_charge_current", "Force Charge Current", "current", "A"
    )
    sensors["qc_force_discharge_current"] = _make_sensor(
        mqtt_settings, "qc_force_discharge_current", "Force Discharge Current", "current", "A"
    )
    return sensors


def create_interactive_entities(mqtt_host, mqtt_port, mqtt_user, mqtt_password, command_handler):
    """Create interactive entities, each with its own MQTT connection.

    Subscriber entities need separate connections because ha-mqtt-discoverable
    sets client.on_message globally per client.
    """
    def make_mqtt():
        return Settings.MQTT(
            host=mqtt_host, port=mqtt_port,
            username=mqtt_user, password=mqtt_password,
        )

    entities = {}

    entities["schedule_enabled"] = Switch(
        Settings(mqtt=make_mqtt(), entity=SwitchInfo(
            name="Schedule Enabled",
            device=DEVICE_INFO,
            unique_id="qc_schedule_enabled",
        )),
        command_callback=command_handler.on_schedule_enabled,
    )

    entities["min_soc"] = Number(
        Settings(mqtt=make_mqtt(), entity=NumberInfo(
            name="Min SoC",
            device=DEVICE_INFO,
            unique_id="qc_min_soc",
            min=0, max=100, step=1,
            unit_of_measurement="%",
            mode="slider",
        )),
        command_callback=command_handler.on_min_soc,
    )

    entities["max_soc"] = Number(
        Settings(mqtt=make_mqtt(), entity=NumberInfo(
            name="Max SoC",
            device=DEVICE_INFO,
            unique_id="qc_max_soc",
            min=0, max=100, step=1,
            unit_of_measurement="%",
            mode="slider",
        )),
        command_callback=command_handler.on_max_soc,
    )

    for i in range(1, SCHEDULE_COUNT + 1):
        entities[f"schedule_{i}_mode"] = Select(
            Settings(mqtt=make_mqtt(), entity=SelectInfo(
                name=f"Schedule {i} Mode",
                device=DEVICE_INFO,
                unique_id=f"qc_schedule_{i}_mode",
                options=MODE_OPTIONS,
            )),
            command_callback=command_handler.make_schedule_mode_callback(i),
        )

        entities[f"schedule_{i}_start"] = Text(
            Settings(mqtt=make_mqtt(), entity=TextInfo(
                name=f"Schedule {i} Start",
                device=DEVICE_INFO,
                unique_id=f"qc_schedule_{i}_start",
                min=8, max=8,
                pattern=r"^\d{2}:\d{2}:\d{2}$",
            )),
            command_callback=command_handler.make_schedule_start_callback(i),
        )

        entities[f"schedule_{i}_end"] = Text(
            Settings(mqtt=make_mqtt(), entity=TextInfo(
                name=f"Schedule {i} End",
                device=DEVICE_INFO,
                unique_id=f"qc_schedule_{i}_end",
                min=8, max=8,
                pattern=r"^\d{2}:\d{2}:\d{2}$",
            )),
            command_callback=command_handler.make_schedule_end_callback(i),
        )

    return entities


def update_sensors_from_data(sensors, data):
    """Update read-only sensors from fetch_qc_data() result."""
    for api_key, (uid, _name, _dc, _unit) in STATUS_KEY_MAP.items():
        if api_key in data["status"]:
            sensors[uid].set_state(data["status"][api_key])
        else:
            logger.debug("Status key '%s' not found in API response", api_key)

    for api_key, (uid, _name, _dc, _unit) in ENERGY_KEY_MAP.items():
        if api_key in data["energy"]:
            sensors[uid].set_state(data["energy"][api_key])
        else:
            logger.debug("Energy key '%s' not found in API response", api_key)

    mapped_status_keys = set(STATUS_KEY_MAP.keys())
    for key in data["status"]:
        if key not in mapped_status_keys:
            logger.debug("Unmapped status key: '%s' = %s", key, data["status"][key])

    mapped_energy_keys = set(ENERGY_KEY_MAP.keys())
    for key in data["energy"]:
        if key not in mapped_energy_keys:
            logger.debug("Unmapped energy key: '%s' = %s", key, data["energy"][key])


def update_entities_from_schedule(sensors, entities, config):
    """Update entity states from DailyScheduleConfig to keep HA in sync."""
    sensors["qc_force_charge_current"].set_state(config.force_charge_current)
    sensors["qc_force_discharge_current"].set_state(config.force_discharge_current)

    if config.state_enabled:
        entities["schedule_enabled"].on()
    else:
        entities["schedule_enabled"].off()

    entities["min_soc"].set_value(config.min_soc)
    entities["max_soc"].set_value(config.max_soc)

    for i, sched in enumerate(config.schedules, 1):
        mode_name = SCHEDULE_MODE_NAMES[sched.mode]
        entities[f"schedule_{i}_mode"].select_option(mode_name)
        entities[f"schedule_{i}_start"].set_text(sched.start_time)
        entities[f"schedule_{i}_end"].set_text(sched.end_time)
