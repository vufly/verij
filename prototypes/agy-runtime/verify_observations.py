#!/usr/bin/env python3
"""Check real G3 metadata fixtures and recorded limitations, without running Agy."""
import json
from pathlib import Path


def main():
    directory = Path(__file__).resolve().parent
    expected = {"turn-results.json": "PASS", "permission-results.json": "PASS",
                "question-results.json": "LIMITATION", "cancel-results.json": "LIMITATION",
                "permission-reply-results.json": "PASS", "background-results.json": "PASS",
                "pretool-rejection-results.json": "LIMITATION", "deadline-results.json": "LIMITATION"}
    cases = {}
    for name, status in expected.items():
        report = json.loads((directory / name).read_text())
        assert report["agy_version"] == "1.2.14" and report["status"] == status
        assert report["private_auth_and_app_data_removed"] and not report["no_model_invocation"]
        case = report["cases"][0]
        cases["pretool-rejection" if name == "pretool-rejection-results.json" else case["scenario"]] = case
        assert case["terminal_transport"] == ("headless stream-json" if case["scenario"] == "deadline" else "native private tmux")
        assert not case["permission_autoapproval"] and case["trusted_workspace_is_generated_probe_only"]
        if case["scenario"] != "deadline":
            assert case["unchecked_native_consent_before_confirmation"]
        root = Path(report["artifacts_dir"]) / case["case"]
        callbacks = [json.loads(line) for line in (root / "statusline.ndjson").read_text().splitlines()]
        hooks = [json.loads(line) for line in (root / "lifecycle.ndjson").read_text().splitlines()]
        # Result is an observation snapshot before teardown. Native close/kill
        # can append callbacks afterward; do not rewrite them as turn evidence.
        sampled_count = case.get("callback_count", len(case["callbacks"]))
        assert sampled_count <= len(callbacks)
        observed = callbacks[:sampled_count]
        assert all(item in observed for item in case["callbacks"])
        assert hooks[:len(case["lifecycle"])] == case["lifecycle"]
        assert all(item["prior_stdout_preserved"] and item["prior_command_exit"] == 0 for item in callbacks)
        assert all(not (root / folder).exists() for folder in ("home", "config", "cache", "data", "state"))
        configuration = json.loads((root / ".agents/hooks.json").read_text())["verij-neutral-probe"]
        assert ("PreToolUse" in configuration) == case.get("empty_pretool_observer_installed", False)
    turn = cases["no-tool"]
    assert "working" in turn["observed_states"] and turn["idle_observed"]
    assert [event["phase"] for event in turn["lifecycle"]] == ["PreInvocation", "PostInvocation", "Stop"]
    stop = turn["lifecycle"][-1]
    assert stop["fullyIdle"] is True and not stop["error_present"] and stop["terminationReason"] == "NO_TOOL_CALL"
    assert len({event["conversation"] for event in turn["lifecycle"]}) == 1
    permission = cases["permission"]
    assert any(item.get("tool_confirmation_pending") is True for item in permission["callbacks"])
    assert [event["phase"] for event in permission["lifecycle"]] == ["PreInvocation"]
    question = cases["question"]
    assert question["test_only_native_question_choices_visible"]
    assert not any(item.get("tool_confirmation_pending") or item.get("pending_input_count", 0) > 0 for item in question["callbacks"])
    assert question["observed_states"][-1] == "working"
    cancel = cases["cancel"]
    assert cancel["cancellation_key_sent_after_working_callback"] and cancel["idle_observed"]
    assert not any(event["phase"] == "Stop" for event in cancel["lifecycle"])
    reply = cases["permission-reply"]
    assert reply["exact_probe_command_native_once_reply"] and not reply["empty_pretool_observer_installed"]
    assert reply["test_only_exact_modal_shape"]["exact_summary"]
    assert any(event["phase"] == "PostToolUse" and not event["error_present"] for event in reply["lifecycle"])
    assert reply["lifecycle"][-1]["fullyIdle"] is True
    background = cases["background"]
    assert background["exact_probe_command_native_once_reply"] and not background["empty_pretool_observer_installed"]
    stops = [event for event in background["lifecycle"] if event["phase"] == "Stop"]
    assert [event["fullyIdle"] for event in stops] == [False, True]
    assert stops[0]["time_ns"] < stops[1]["time_ns"]
    assert stops[0]["conversation"] == stops[1]["conversation"]
    assert any(item["agent_state"] == "idle" and item.get("task_count") == 1 for item in background["callbacks"])
    assert any(stops[0]["time_ns"] < event["time_ns"] < stops[1]["time_ns"] and event["phase"] == "PostToolUse" for event in background["lifecycle"])
    experiment = cases["pretool-rejection"]
    assert experiment["empty_pretool_observer_installed"] and not experiment["exact_probe_command_native_once_reply"]
    assert any(event["phase"] == "PreToolUse" and event["tool_name"] == "run_command" and event["exact_permission_probe_command"] for event in experiment["lifecycle"])
    assert not any(item.get("tool_confirmation_pending") for item in experiment["callbacks"])
    assert not any(event["phase"] == "PostToolUse" for event in experiment["lifecycle"])
    deadline = cases["deadline"]
    assert deadline["final_result_status"] == "SUCCESS" and deadline["cli_exit_code"] == 0
    assert deadline["final_response_characters"] == 0
    assert [event["phase"] for event in deadline["lifecycle"]] == ["PreInvocation"]
    assert deadline["callbacks"]  # Headless startup callback execution is observed.
    print(json.dumps({"verified_cases": expected, "original_metadata_matches": True,
                      "question_and_cancel_not_inferred_as_success": True, "h0_approved": False}, indent=2))


if __name__ == "__main__":
    main()
