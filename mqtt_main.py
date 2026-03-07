import argparse
import logging
import os
import signal
import time

import paho.mqtt.client as mqtt
from ha_mqtt_discoverable import Settings

from mqtt_commands import CommandHandler
from mqtt_entities import (
    create_sensors,
    create_interactive_entities,
    update_sensors_from_data,
    update_entities_from_schedule,
)
from qcore import fetch_qc_data, fetch_daily_schedule

logger = logging.getLogger(__name__)


def resolve_password(value):
    """If value is a path to an existing file, read password from it. Otherwise return as-is."""
    if os.path.isfile(value):
        with open(value) as f:
            return f.read().strip()
    return value


def main():
    parser = argparse.ArgumentParser(description="Qendercore MQTT adapter for Home Assistant")
    parser.add_argument("--qc-login", required=True, help="Qendercore account login")
    parser.add_argument("--qc-password", required=True, help="Qendercore password or path to password file")
    parser.add_argument("--mqtt-host", required=True, help="MQTT broker host")
    parser.add_argument("--mqtt-port", type=int, default=1883, help="MQTT broker port")
    parser.add_argument("--mqtt-user", required=True, help="MQTT username")
    parser.add_argument("--mqtt-password", required=True, help="MQTT password or path to password file")
    parser.add_argument("--interval", type=int, default=60, help="Polling interval in seconds")
    parser.add_argument("--debug", action="store_true", help="Enable debug logging")

    args = parser.parse_args()

    logging.basicConfig(
        level=logging.DEBUG if args.debug else logging.INFO,
        format="%(asctime)s %(levelname)s %(name)s: %(message)s",
    )

    qc_password = resolve_password(args.qc_password)
    mqtt_password = resolve_password(args.mqtt_password)

    # Shared MQTT client for read-only sensors
    client = mqtt.Client(mqtt.CallbackAPIVersion.VERSION2, client_id="qendercore-adapter")
    client.username_pw_set(args.mqtt_user, mqtt_password)
    client.connect(args.mqtt_host, args.mqtt_port)
    client.loop_start()
    sensor_mqtt = Settings.MQTT(client=client)

    handler = CommandHandler(args.qc_login, qc_password)

    logger.info("Creating MQTT entities...")
    sensors = create_sensors(sensor_mqtt)
    entities = create_interactive_entities(
        args.mqtt_host, args.mqtt_port, args.mqtt_user, mqtt_password, handler,
    )
    handler.entities = entities
    logger.info("Created %d sensors and %d interactive entities", len(sensors), len(entities))

    running = True

    def on_signal(signum, frame):
        nonlocal running
        logger.info("Received signal %d, shutting down...", signum)
        running = False

    signal.signal(signal.SIGINT, on_signal)
    signal.signal(signal.SIGTERM, on_signal)

    logger.info("Starting polling loop (interval=%ds)", args.interval)
    while running:
        try:
            data = fetch_qc_data(args.qc_login, qc_password)
            update_sensors_from_data(sensors, data)
            logger.debug("Updated status/energy sensors")
        except Exception:
            logger.exception("Failed to fetch/update QC data")

        try:
            config = fetch_daily_schedule(args.qc_login, qc_password)
            update_entities_from_schedule(sensors, entities, config)
            logger.debug("Updated schedule entities")
        except Exception:
            logger.exception("Failed to fetch/update schedule")

        for _ in range(args.interval):
            if not running:
                break
            time.sleep(1)

    client.loop_stop()
    client.disconnect()
    logger.info("Stopped")


if __name__ == "__main__":
    main()
