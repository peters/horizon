"""Independent decoding of the actual rendered whole-window grayscale workload."""
import hashlib
import json
import os
import select
from pathlib import Path
import statistics
import subprocess
import time


def gray(frame, width, x, y):
    at = (int(y) * width + int(x)) * 3
    return sum(frame[at:at + 3]) / 3


def chrome_cell(frame, width, x, y, radius):
    """Compare corresponding small areas instead of aliased single-pixel edges."""
    height = len(frame) // (width * 3)
    return statistics.mean(gray(frame, width, px, py)
                           for py in range(max(0, int(y - radius)), min(height, int(y + radius) + 1))
                           for px in range(max(0, int(x - radius)), min(width, int(x + radius) + 1)))


def classify(value):
    return 0 if value < 32 else 255 if value > 223 else None


def inspect_frame(frame, width, height, marker):
    x, y, unit, band = marker
    def value(column, vertical=0):
        px, py = int(x + (column + 0.5) * unit), int(y + vertical)
        if not (0 <= px < width and 0 <= py < height):
            raise RuntimeError('fixture marker extends outside decoded canvas')
        return gray(frame, width, px, py)
    for offset, expected in enumerate([255, 0] * 3):
        if abs(value(offset) - expected) > 35 or abs(value(24 + offset) - (255 - expected)) > 35:
            raise RuntimeError('decoded fixture guards changed or disappeared')
    bits = [classify(value(7 + bit)) for bit in range(16)]
    if None in bits:
        raise RuntimeError('decoded identifier cell quality regressed')
    identifier = sum((bit == 255) << at for at, bit in enumerate(bits))
    errors = []
    # Two rows of four-terminal-row checker tiles, away from tile edges.
    for row in range(2):
        for column in range(8):
            expected = 30 if (column + row) % 2 == 0 else 180
            errors.append(abs(value(2 * column + 0.5, band * (1.5 + row)) - expected))
    if statistics.mean(errors) > 12 or max(errors) > 30:
        raise RuntimeError('decoded grayscale quality regressed')
    return identifier, errors


def locate(frame, width, height, unit_hint=None):
    """Locate six large alternating guards, then validate the independent checker."""
    for y in range(0, height, 2):
        runs = []
        previous, start = None, 0
        for x in range(width + 1):
            kind = classify(gray(frame, width, x, y)) if x < width else None
            if kind != previous:
                if previous is not None:
                    runs.append((start, x, previous))
                previous, start = kind, x
        for at in range(len(runs) - 5):
            six = runs[at:at + 6]
            if [run[2] for run in six] != [255, 0] * 3 or any(six[i][1] != six[i + 1][0] for i in range(5)):
                continue
            unit = unit_hint if unit_hint is not None else statistics.mean(end - start for start, end, _ in six)
            if unit < 3 or any(abs(end - start - unit) > max(2, unit * 0.4) for start, end, _ in six):
                continue
            x = six[0][0]
            sample_x = int(x + unit / 2)
            top, bottom = y, y
            while top > 0 and classify(gray(frame, width, sample_x, top - 1)) == 255:
                top -= 1
            while bottom + 1 < height and classify(gray(frame, width, sample_x, bottom + 1)) == 255:
                bottom += 1
            band = bottom - top + 1
            marker = (x, (top + bottom) / 2, unit, band)
            try:
                inspect_frame(frame, width, height, marker)
                return marker
            except RuntimeError:
                continue
    raise RuntimeError('large fixture guards/checker absent from decoded whole window')


def chrome_reference(reference, scale=1):
    image = Path(reference['image'])
    if hashlib.sha256(image.read_bytes()).hexdigest() != reference['sha256']:
        raise RuntimeError('independent root reference changed')
    width, height = reference['region']['width'], reference['region']['height']
    pixels = subprocess.check_output(['ffmpeg', '-v', 'error', '-i', str(image), '-pix_fmt',
              'rgb24', '-f', 'rawvideo', 'pipe:1'], timeout=30)
    if len(pixels) != width * height * 3:
        raise RuntimeError('root reference dimensions changed')
    marker = locate(pixels, width, height)
    points = [(width * (column + .5) / 16, height * fraction)
              for fraction in [.015, .04, .075] for column in range(16)]
    points += [(width * fraction, height * (row + .5) / 12)
               for fraction in [.01, .04] for row in range(1, 11)]
    values = [chrome_cell(pixels, width, x, y, 1 / scale) for x, y in points]
    if max(values) - min(values) < 12:
        raise RuntimeError('root chrome reference lacks contrasting cells')
    return {'width': width, 'height': height, 'marker': marker, 'points': points, 'values': values}


def inspect_chrome(frame, width, height, marker, reference):
    x, y, unit, band = marker
    rx, ry, ru, rb = reference['marker']
    scale = min(width / reference['width'], height / reference['height'])
    left = (width - reference['width'] * scale) / 2
    top = (height - reference['height'] * scale) / 2
    def project(px, py):
        return left + px * scale, top + py * scale
    expected_x, expected_y = project(rx, ry)
    if (abs(x - expected_x) > 3 or abs(y - expected_y) > 3
            or abs(unit - ru * scale) > 2 or abs(band - rb * scale) > 2):
        raise RuntimeError('decoded marker geometry differs from full root reference')
    for px, py in [(0, 0), (reference['width'] - 1, reference['height'] - 1)]:
        tx, ty = project(px, py)
        if not (-2 <= tx < width + 2 and -2 <= ty < height + 2):
            raise RuntimeError('decoded marker alignment excludes part of root window')
    errors = []
    for point, expected in zip(reference['points'], reference['values']):
        tx, ty = project(*point)
        if not (0 <= tx < width and 0 <= ty < height):
            raise RuntimeError('root chrome outside decoded canvas')
        errors.append(abs(chrome_cell(frame, width, tx, ty, 1) - expected))
    if statistics.mean(errors) > 18 or max(errors) > 70:
        raise RuntimeError('independent root chrome differs outside terminal fixture')
    return statistics.mean(errors)


def read_bounded(process, count, deadline):
    result = bytearray()
    while len(result) < count:
        remaining = deadline - time.monotonic()
        if remaining <= 0 or not select.select([process.stdout], [], [], remaining)[0]:
            raise TimeoutError('independent decoder stalled')
        block = os.read(process.stdout.fileno(), count - len(result))
        if not block:
            break
        result.extend(block)
    return bytes(result)


def measurement_ids(identifiers, state, measurement):
    if measurement is None:
        return identifiers
    times = state.get('frame_received_times', [])
    if len(times) != len(identifiers) or any(a >= b for a, b in zip(times, times[1:])):
        raise RuntimeError('receiver timestamps do not cover decoded frames in order')
    began, ended = measurement['began_monotonic'], measurement['ended_monotonic']
    if not began < ended:
        raise RuntimeError('fixed measurement interval is invalid')
    measured = [identifier for identifier, stamp in zip(identifiers, times) if began <= stamp <= ended]
    if len(set(measured)) < 2:
        raise RuntimeError('frozen or missing content during fixed measurement window')
    return measured


def decode(root, state, expected_canvas, reference=None, measurement=None):
    root = Path(root)
    stream = root / state['stream_file']
    probe = subprocess.run(['ffprobe', '-v', 'error', '-count_frames', '-show_streams', '-of', 'json', str(stream)],
                           check=True, capture_output=True, text=True, timeout=180)
    info = json.loads(probe.stdout)['streams'][0]
    if (info['width'], info['height']) != tuple(expected_canvas):
        raise RuntimeError('decoded canvas differs from requested resolution/orientation')
    if state.get('configs', 0) < 1 or len(state.get('config_dimensions', [])) != state['configs'] * 3:
        raise RuntimeError('video configuration coverage missing')
    if any(tuple(size) != tuple(expected_canvas) for size in state['config_dimensions']):
        raise RuntimeError('video configuration changed output canvas')
    if int(info['nb_read_frames']) != state['frames'] or state['frames'] < 2:
        raise RuntimeError('independent frame count does not match receiver')
    graph = 'scale=w=960:h=960:force_original_aspect_ratio=decrease:force_divisible_by=2:flags=neighbor'
    dimensions = [info['width'], info['height']]
    ratio = min(960 / dimensions[0], 960 / dimensions[1])
    width, height = [int(value * ratio) // 2 * 2 for value in dimensions]
    chrome = chrome_reference(reference, min(width / reference['region']['width'],
                                            height / reference['region']['height'])) if reference else None
    diagnostic_path = root / (stream.stem + '-ffmpeg.log')
    diagnostic_log = diagnostic_path.open('wb')
    process = subprocess.Popen(['ffmpeg', '-v', 'error', '-i', str(stream), '-vf', graph,
                                '-pix_fmt', 'rgb24', '-f', 'rawvideo', 'pipe:1'], stdout=subprocess.PIPE,
                               stderr=diagnostic_log)
    identifiers, errors, chrome_errors, marker = [], [], [], None
    unit_hint = chrome['marker'][2] * min(width / chrome['width'], height / chrome['height']) if chrome else None
    deadline = time.monotonic() + 180
    try:
        for _ in range(state['frames']):
            frame = read_bounded(process, width * height * 3, deadline)
            if len(frame) != width * height * 3:
                raise RuntimeError('truncated independent decoded image coverage')
            marker = marker or locate(frame, width, height, unit_hint)
            identifier, quality = inspect_frame(frame, width, height, marker)
            if identifiers and identifier < identifiers[-1]:
                raise RuntimeError('fixture IDs reversed or wrapped')
            identifiers.append(identifier)
            errors.extend(quality)
            if chrome:
                chrome_errors.append(inspect_chrome(frame, width, height, marker, chrome))
        if read_bounded(process, 1, deadline):
            raise RuntimeError('decoder produced unexpected extra frames')
        returncode = process.wait(timeout=10)
        diagnostic_log.flush()
        diagnostic = diagnostic_path.read_text()
        if returncode != 0 or diagnostic.strip():
            raise RuntimeError('independent decoder errors: ' + diagnostic)
    finally:
        if process.poll() is None:
            process.kill()
        process.wait(timeout=10)
        diagnostic_log.close()
    distinct = len(set(identifiers))
    if distinct < 2:
        raise RuntimeError('frozen content cannot qualify whole-window capture')
    measured = measurement_ids(identifiers, state, measurement)
    result = {'measured_decoded_frames': len(measured),
              'measured_content_updates': sum(a != b for a, b in zip(measured, measured[1:])),
              'measured_duplicate_frames': sum(a == b for a, b in zip(measured, measured[1:])),
              'decoded_frames': len(identifiers), 'distinct_content_ids': distinct,
              'first_id': identifiers[0], 'last_id': identifiers[-1], 'marker': marker,
              'root_chrome_reference': bool(chrome),
              'mean_root_chrome_error': statistics.mean(chrome_errors) if chrome_errors else None,
              'mean_grayscale_error': statistics.mean(errors), 'max_grayscale_error': max(errors),
              'pending_configuration_bytes': state.get('pending_configuration_bytes', 0)}
    (root / (stream.stem + '-decode.json')).write_text(json.dumps(result, indent=2))
    return result
