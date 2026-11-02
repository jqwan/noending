import Icon from "../../components/Icon";
import CustomSelect from "../../components/CustomSelect";

export default function CollaborationCoordinator({ value, members, disabled, onChange }: {
  value: string | null;
  members: { id: string; title: string }[];
  disabled: boolean;
  onChange: (id: string | null) => void;
}) {
  return (
    <div className="collab-coordinator small">
      <Icon name="spark" />
      <span>负责人</span>
      <CustomSelect ariaLabel="负责人" value={value ?? ""} disabled={disabled || members.length === 0}
        onChange={value => onChange(value || null)}
        options={[{ value: "", label: "由我协调" }, ...members.map(member => ({ value: member.id, label: member.title }))]} />
    </div>
  );
}
