import logging
import threading

from qcore import (
    fetch_daily_schedule,
    set_daily_schedule,
    SCHEDULE_MODE_NAMES,
)

logger = logging.getLogger(__name__)

MODE_NAME_TO_INT = {v: k for k, v in SCHEDULE_MODE_NAMES.items()}


class CommandHandler:
    def __init__(self, qc_login, qc_password):
        self.qc_login = qc_login
        self.qc_password = qc_password
        self.lock = threading.Lock()
        self.entities = {}

    def _apply(self, description, modify_fn, confirm_fn):
        with self.lock:
            try:
                config = fetch_daily_schedule(self.qc_login, self.qc_password)
                modify_fn(config)
                set_daily_schedule(self.qc_login, self.qc_password, config)
                confirm_fn(config)
                logger.info("Applied: %s", description)
            except Exception:
                logger.exception("Failed to apply: %s", description)

    def on_schedule_enabled(self, client, user_data, msg):
        payload = msg.payload.decode()

        def modify(config):
            config.state_enabled = (payload == "ON")

        def confirm(config):
            if config.state_enabled:
                self.entities["schedule_enabled"].on()
            else:
                self.entities["schedule_enabled"].off()

        self._apply(f"schedule_enabled={payload}", modify, confirm)

    def on_min_soc(self, client, user_data, msg):
        value = int(float(msg.payload.decode()))

        def modify(config):
            config.min_soc = value

        def confirm(_config):
            self.entities["min_soc"].set_value(value)

        self._apply(f"min_soc={value}", modify, confirm)

    def on_max_soc(self, client, user_data, msg):
        value = int(float(msg.payload.decode()))

        def modify(config):
            config.max_soc = value

        def confirm(_config):
            self.entities["max_soc"].set_value(value)

        self._apply(f"max_soc={value}", modify, confirm)

    def make_schedule_mode_callback(self, slot):
        def callback(client, user_data, msg):
            option = msg.payload.decode()
            mode = MODE_NAME_TO_INT[option]

            def modify(config):
                config.schedules[slot - 1].mode = mode

            def confirm(_config):
                self.entities[f"schedule_{slot}_mode"].select_option(option)

            self._apply(f"schedule_{slot}_mode={option}", modify, confirm)

        return callback

    def make_schedule_start_callback(self, slot):
        def callback(client, user_data, msg):
            text = msg.payload.decode()

            def modify(config):
                config.schedules[slot - 1].start_time = text

            def confirm(_config):
                self.entities[f"schedule_{slot}_start"].set_text(text)

            self._apply(f"schedule_{slot}_start={text}", modify, confirm)

        return callback

    def make_schedule_end_callback(self, slot):
        def callback(client, user_data, msg):
            text = msg.payload.decode()

            def modify(config):
                config.schedules[slot - 1].end_time = text

            def confirm(_config):
                self.entities[f"schedule_{slot}_end"].set_text(text)

            self._apply(f"schedule_{slot}_end={text}", modify, confirm)

        return callback
