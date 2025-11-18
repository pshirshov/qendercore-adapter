import argparse
import json

import qcore


def load_credentials():
    with open('auth.json') as f:
        data = json.loads(f.read())
        return data['login'], data['password']


def cmd_status(args):
    """Show current inverter status and schedules"""
    login, password = load_credentials()

    # Fetch dashboard data
    data = qcore.fetch_qc_data(login, password)

    print("=== Current Status ===\n")
    for key, value in data['status'].items():
        print(f"{key}: {value}")

    print("\n=== Energy ===\n")
    for key, value in data['energy'].items():
        print(f"{key}: {value}")

    # Fetch and display schedule
    print("\n=== Daily Schedule Configuration ===\n")
    config = qcore.fetch_daily_schedule(login, password)
    print(config)


def cmd_schedule(args):
    """Show current schedule only"""
    login, password = load_credentials()

    config = qcore.fetch_daily_schedule(login, password)
    print(config)


def cmd_set_soc(args):
    """Set min/max battery SoC"""
    login, password = load_credentials()

    # Fetch current config
    config = qcore.fetch_daily_schedule(login, password)

    # Update SoC values
    if args.min is not None:
        assert 0 <= args.min <= 100, f"Min SoC must be between 0 and 100, got {args.min}"
        config.min_soc = args.min

    if args.max is not None:
        assert 0 <= args.max <= 100, f"Max SoC must be between 0 and 100, got {args.max}"
        config.max_soc = args.max

    assert config.min_soc <= config.max_soc, f"Min SoC ({config.min_soc}) must be <= Max SoC ({config.max_soc})"

    # Apply changes
    print(f"Setting Min SoC: {config.min_soc}%, Max SoC: {config.max_soc}%")
    response = qcore.set_daily_schedule(login, password, config)

    if "msgs" in response and response["msgs"]:
        for msg in response["msgs"]:
            print(f"Response: {msg['msg']}")
    else:
        print(f"Response: {json.dumps(response, indent=2)}")


def cmd_set_schedule(args):
    """Set a specific schedule slot"""
    login, password = load_credentials()

    # Fetch current config
    config = qcore.fetch_daily_schedule(login, password)

    # Parse mode
    mode_map = {
        'disable': qcore.ScheduleMode.DISABLE,
        'off': qcore.ScheduleMode.DISABLE,
        'charge': qcore.ScheduleMode.FORCED_CHARGE,
        'discharge': qcore.ScheduleMode.FORCED_DISCHARGE,
    }

    mode_str = args.mode.lower()
    assert mode_str in mode_map, f"Invalid mode '{args.mode}'. Use: disable, charge, discharge"
    mode = mode_map[mode_str]

    # Validate slot number
    slot = args.slot
    assert 1 <= slot <= 5, f"Slot must be between 1 and 5, got {slot}"

    # Update the schedule slot
    if mode == qcore.ScheduleMode.DISABLE:
        config.schedules[slot - 1] = qcore.Schedule(
            mode=mode,
            start_time="00:00:00",
            end_time="00:00:00"
        )
        print(f"Disabling schedule slot {slot}")
    else:
        assert args.start, "Start time required for charge/discharge mode"
        assert args.end, "End time required for charge/discharge mode"

        # Normalize time format
        start = args.start if len(args.start) == 8 else args.start + ":00"
        end = args.end if len(args.end) == 8 else args.end + ":00"

        config.schedules[slot - 1] = qcore.Schedule(
            mode=mode,
            start_time=start,
            end_time=end
        )
        mode_name = "Forced Charge" if mode == qcore.ScheduleMode.FORCED_CHARGE else "Forced Discharge"
        print(f"Setting schedule slot {slot}: {mode_name} {start} - {end}")

    # Apply changes
    response = qcore.set_daily_schedule(login, password, config)

    if "msgs" in response and response["msgs"]:
        for msg in response["msgs"]:
            print(f"Response: {msg['msg']}")
    else:
        print(f"Response: {json.dumps(response, indent=2)}")


def cmd_enable_schedule(args):
    """Enable or disable the schedule system"""
    login, password = load_credentials()

    config = qcore.fetch_daily_schedule(login, password)
    config.state_enabled = args.state == 'on'

    state_str = "On" if config.state_enabled else "Off"
    print(f"Setting schedule state: {state_str}")

    response = qcore.set_daily_schedule(login, password, config)

    if "msgs" in response and response["msgs"]:
        for msg in response["msgs"]:
            print(f"Response: {msg['msg']}")
    else:
        print(f"Response: {json.dumps(response, indent=2)}")


def main():
    parser = argparse.ArgumentParser(
        description='Qendercore Inverter Adapter',
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
  %(prog)s status                          Show inverter status and schedules
  %(prog)s schedule                        Show current schedule only
  %(prog)s set-soc --min 15 --max 100      Set battery SoC limits
  %(prog)s set-schedule 1 charge 02:00 05:00   Set slot 1 to charge 2am-5am
  %(prog)s set-schedule 2 discharge 17:00 20:00  Set slot 2 to discharge 5pm-8pm
  %(prog)s set-schedule 3 disable          Disable slot 3
  %(prog)s enable on                       Enable schedule system
  %(prog)s enable off                      Disable schedule system
"""
    )

    subparsers = parser.add_subparsers(dest='command', help='Command to run')

    # status command
    status_parser = subparsers.add_parser('status', help='Show inverter status and schedules')
    status_parser.set_defaults(func=cmd_status)

    # schedule command
    schedule_parser = subparsers.add_parser('schedule', help='Show current schedule configuration')
    schedule_parser.set_defaults(func=cmd_schedule)

    # set-soc command
    soc_parser = subparsers.add_parser('set-soc', help='Set battery SoC limits')
    soc_parser.add_argument('--min', type=int, help='Minimum battery SoC (0-100)')
    soc_parser.add_argument('--max', type=int, help='Maximum battery SoC (0-100)')
    soc_parser.set_defaults(func=cmd_set_soc)

    # set-schedule command
    set_sched_parser = subparsers.add_parser('set-schedule', help='Set a schedule slot')
    set_sched_parser.add_argument('slot', type=int, help='Schedule slot number (1-5)')
    set_sched_parser.add_argument('mode', help='Mode: charge, discharge, or disable')
    set_sched_parser.add_argument('start', nargs='?', help='Start time (HH:MM or HH:MM:SS)')
    set_sched_parser.add_argument('end', nargs='?', help='End time (HH:MM or HH:MM:SS)')
    set_sched_parser.set_defaults(func=cmd_set_schedule)

    # enable command
    enable_parser = subparsers.add_parser('enable', help='Enable or disable schedule system')
    enable_parser.add_argument('state', choices=['on', 'off'], help='on or off')
    enable_parser.set_defaults(func=cmd_enable_schedule)

    args = parser.parse_args()

    if args.command is None:
        # Default to status if no command given
        args.func = cmd_status

    args.func(args)


if __name__ == "__main__":
    main()
